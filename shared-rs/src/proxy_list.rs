// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Reading a pasted block of proxies.
//!
//! Nobody types proxies in one at a time. They arrive as a block from a
//! supplier, in whichever of half a dozen shapes that supplier prefers, and the
//! only thing they reliably have in common is one per line.
//!
//! # Why this is more dangerous than it looks
//!
//! A parser that guesses wrong does not fail — it succeeds, and sends an
//! account's traffic through a host that was meant to be a username. The
//! account is then browsing from somewhere it has never been, which is the
//! exact event every part of this product exists to prevent. So:
//!
//!   - Every line is parsed independently and reported independently. One bad
//!     line does not abandon the block, and does not silently vanish from it.
//!   - Nothing is inferred from a line that could be read two ways without
//!     saying which reading was taken. `ParsedProxy` carries the shape it
//!     matched so the UI can show it back before anything is saved.
//!   - An unsupported scheme is an error, not a downgrade. Quietly treating
//!     socks4 as socks5 would produce a proxy that fails at launch, hours later,
//!     with nothing pointing back at this moment.

use std::fmt;

/// One proxy, parsed but not yet checked or stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedProxy {
    /// "http" | "https" | "socks5"
    ///
    /// socks5h collapses to socks5: the relay resolves names remotely
    /// unconditionally, because local resolution would leak the operator's
    /// resolver. Keeping the distinction in the model would imply a choice
    /// there is not.
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Which shape this line matched. Shown back to the operator, because the
    /// difference between `host:port:user:pass` and `user:pass:host:port` is
    /// invisible in the result and catastrophic when guessed wrong.
    pub shape: Shape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `scheme://host:port`, with or without `user:pass@`.
    Url,
    /// `host:port`, no credentials.
    HostPort,
    /// `host:port:user:pass` — what most suppliers hand out.
    HostPortUserPass,
    /// `user:pass@host:port` with no scheme.
    AtSign,
}

impl ParsedProxy {
    /// The form `parse_upstream` and the store both accept.
    pub fn url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => {
                format!("{}://{}:{}@{}:{}", self.scheme, u, p, self.host, self.port)
            }
            _ => format!("{}://{}:{}", self.scheme, self.host, self.port),
        }
    }
}

/// Why a line could not be read.
///
/// Carries a stable `code` as well as the sentence. The sentence is written
/// here because this crate is where the reason is actually known, and it is
/// what a script or the CLI prints; the code is what lets the desktop app say
/// the same thing in the operator's own language. Without it the app would show
/// an English sentence beside a Russian interface, which is how the rest of the
/// notices got complained about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn err(code: &'static str, m: impl Into<String>) -> ParseError {
    ParseError { code, message: m.into() }
}

/// One entry per non-empty, non-comment line, in the order they appeared.
///
/// The line number is 1-based and counts every line including the skipped ones,
/// so it matches what the operator sees in the box they pasted into.
pub fn parse_block(text: &str) -> Vec<(usize, Result<ParsedProxy, ParseError>)> {
    text.lines()
        .enumerate()
        .filter_map(|(i, raw)| {
            let line = raw.trim();
            // Blank lines and comments are how people annotate a supplier's
            // list. Dropping them silently is right; dropping a real line
            // silently is not, which is why nothing else is filtered here.
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                return None;
            }
            Some((i + 1, parse_line(line)))
        })
        .collect()
}

/// Parse one line.
pub fn parse_line(line: &str) -> Result<ParsedProxy, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(err("empty", "empty line"));
    }

    // A CSV row from a supplier's export, converted to the colon form this
    // already understands.
    //
    // Suppliers send a pasted block or a downloaded .csv and nothing else, and
    // the block was the only shape that worked — so anybody with a .csv opened
    // it, deleted the header, and replaced every comma by hand. The fields are
    // in the same order either way, which is what makes this a rewrite rather
    // than a second parser: host,port[,user,pass] becomes host:port[:user:pass]
    // and falls through to the code below.
    //
    // Deliberately narrow. A line is treated as CSV only when it has commas,
    // no colons and no scheme — so `socks5://user:pass@host:1080` and
    // `host:1080:user:pass` are untouched, and a password containing a comma
    // still arrives intact through the colon form, which is the shape that
    // supports it.
    if line.contains(',') && !line.contains(':') && !line.contains("://") {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        // A header row rather than a proxy. Skipped by name rather than by
        // position: exports put the columns in any order and some have no
        // header at all.
        let first = fields[0].to_ascii_lowercase();
        if matches!(first.as_str(), "host" | "ip" | "address" | "server" | "proxy") {
            return Err(err("header", "this looks like a CSV header, not a proxy"));
        }
        if matches!(fields.len(), 2 | 4) {
            return parse_line(&fields.join(":"));
        }
        return Err(err(
            "csv-fields",
            "a CSV row needs host,port or host,port,user,pass",
        ));
    }

    if let Some((scheme, rest)) = line.split_once("://") {
        let scheme = normalise_scheme(scheme)?;
        let (username, password, hostport) = split_at_sign(rest)?;
        let (host, port) = split_host_port(hostport)?;
        return Ok(ParsedProxy { scheme, host, port, username, password, shape: Shape::Url });
    }

    if line.contains('@') {
        let (username, password, hostport) = split_at_sign(line)?;
        let (host, port) = split_host_port(hostport)?;
        return Ok(ParsedProxy {
            // No scheme means HTTP. Not a guess: every supplier that omits the
            // scheme is selling an HTTP proxy, and a SOCKS one is always
            // labelled, because it has to be.
            scheme: "http".into(),
            host,
            port,
            username,
            password,
            shape: Shape::AtSign,
        });
    }

    // Colon-separated. An IPv6 literal has to be bracketed to get here, which
    // is also the only way it is unambiguous — `::1:8080` has no reading a
    // machine can be sure of, and neither does a person.
    let parts = split_colons(line)?;
    match parts.len() {
        2 => {
            let (host, port) = (parts[0].clone(), parse_port(&parts[1])?);
            Ok(ParsedProxy {
                scheme: "http".into(),
                host,
                port,
                username: None,
                password: None,
                shape: Shape::HostPort,
            })
        }
        4 => {
            // The ambiguity that matters. `a:b:c:d` is host:port:user:pass for
            // essentially every supplier, and user:pass:host:port for a few.
            // Both readings can parse.
            //
            // The rule: field 2 must be a port. If it is not, but field 4 is,
            // the line is the other order and is REFUSED rather than
            // reinterpreted — because a line that needs reinterpreting is a
            // line the operator should look at, and a wrong exit is not
            // something to be clever about.
            let port = parts[1].parse::<u16>().ok();
            let tail_port = parts[3].parse::<u16>().ok();
            match (port, tail_port) {
                (Some(p), _) if p != 0 => Ok(ParsedProxy {
                    scheme: "http".into(),
                    host: parts[0].clone(),
                    port: p,
                    username: Some(parts[2].clone()),
                    password: Some(parts[3].clone()),
                    shape: Shape::HostPortUserPass,
                }),
                (_, Some(_)) => Err(err(
                    "reversed",
                    "this looks like user:pass:host:port. Fury reads four fields as \
                     host:port:user:pass, so rewrite the line that way or use \
                     http://user:pass@host:port",
                )),
                _ => Err(err(
                    "notAPort",
                    format!("expected host:port:user:pass, and {:?} is not a port", parts[1]),
                )),
            }
        }
        3 => Err(err(
            "threeFields",
            "three fields is not a shape Fury reads. Use host:port, \
             host:port:user:pass, or http://user:pass@host:port",
        )),
        n => Err(err(
            "wrongFieldCount",
            format!(
                "{n} fields separated by colons. Use host:port, host:port:user:pass, \
                 or http://user:pass@host:port"
            ),
        )),
    }
}

fn normalise_scheme(scheme: &str) -> Result<String, ParseError> {
    match scheme.trim().to_ascii_lowercase().as_str() {
        "http" => Ok("http".into()),
        "https" => Ok("https".into()),
        // See the note on `ParsedProxy::scheme`: the relay always resolves
        // remotely, so these are the same thing here.
        "socks5" | "socks5h" => Ok("socks5".into()),
        "socks4" | "socks4a" => Err(err(
            "socks4",
            "socks4 has no authentication and no remote DNS, so a profile using one \
             would resolve names on this machine and leak the operator's resolver. \
             Use socks5",
        )),
        other => Err(err(
            "badScheme",
            format!("{other:?} is not a proxy scheme Fury speaks. Use http, https or socks5"),
        )),
    }
}

/// Split `user:pass@host:port`, tolerating an absent credential part.
fn split_at_sign(rest: &str) -> Result<(Option<String>, Option<String>, &str), ParseError> {
    let Some((auth, hostport)) = rest.rsplit_once('@') else {
        return Ok((None, None, rest));
    };
    // First colon, not last: a password may contain colons and a username may
    // not. Splitting the other way would move part of the password into the
    // username and authenticate as nobody.
    let (u, p) = auth
        .split_once(':')
        .ok_or_else(|| err("badCredentials", "credentials before @ must be user:pass"))?;
    if u.is_empty() {
        return Err(err("emptyUsername", "the username before @ is empty"));
    }
    Ok((Some(u.to_string()), Some(p.to_string()), hostport))
}

fn split_host_port(hostport: &str) -> Result<(String, u16), ParseError> {
    // Bracketed IPv6 first: `[2001:db8::1]:1080` has colons inside the host, so
    // the plain rsplit below would read one of those as the port separator.
    if let Some(inner) = hostport.strip_prefix('[') {
        let (host, rest) = inner
            .split_once(']')
            .ok_or_else(|| err("unclosedBracket", "an IPv6 address needs its closing bracket: [address]:port"))?;
        let port = rest
            .strip_prefix(':')
            .ok_or_else(|| err("noPortV6", "an IPv6 proxy needs a port: [address]:port"))?;
        return Ok((host.to_string(), parse_port(port)?));
    }

    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| err("noPort", format!("{hostport:?} has no port")))?;
    if host.is_empty() {
        return Err(err("emptyHost", "the host is empty"));
    }
    Ok((host.to_string(), parse_port(port)?))
}

fn parse_port(s: &str) -> Result<u16, ParseError> {
    let port: u16 = s
        .trim()
        .parse()
        .map_err(|_| err("notANumber", format!("{s:?} is not a port number")))?;
    if port == 0 {
        return Err(err("zeroPort", "port 0 is not a port a proxy can listen on"));
    }
    Ok(port)
}

fn split_colons(line: &str) -> Result<Vec<String>, ParseError> {
    if line.starts_with('[') {
        // `[v6]:port` reaches here only with no credentials; anything more is a
        // URL and was handled above.
        let (host, port) = split_host_port(line)?;
        return Ok(vec![host, port.to_string()]);
    }
    Ok(line.split(':').map(|s| s.trim().to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(line: &str) -> ParsedProxy {
        parse_line(line).unwrap_or_else(|e| panic!("{line:?} should parse: {e}"))
    }

    #[test]
    fn a_csv_row_is_the_colon_form_with_commas() {
        // What a supplier's .csv export actually contains. Before this, the only
        // way in was to open it and replace every comma by hand.
        let p = parse_line("203.0.113.7,8080").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.scheme.as_str()), ("203.0.113.7", 8080, "http"));

        let p = parse_line("203.0.113.7,8080,user278406,secret").unwrap();
        assert_eq!(p.username.as_deref(), Some("user278406"));
        assert_eq!(p.password.as_deref(), Some("secret"));

        // Spaces after the commas are what a spreadsheet writes.
        let p = parse_line("203.0.113.7, 8080, u, p").unwrap();
        assert_eq!((p.port, p.username.as_deref()), (8080, Some("u")));

        // A header row is named rather than counted, because exports order
        // their columns differently and some have no header at all.
        assert_eq!(parse_line("host,port,username,password").unwrap_err().code, "header");
        assert_eq!(parse_line("IP,Port").unwrap_err().code, "header");

        // Three fields is not a shape anybody sends, and guessing which one is
        // missing would put a password in a username.
        assert_eq!(parse_line("203.0.113.7,8080,user").unwrap_err().code, "csv-fields");
    }

    #[test]
    fn a_comma_does_not_capture_the_shapes_that_own_it() {
        // A password may contain a comma, and the colon form is where that is
        // expressible. The CSV branch must not touch these.
        let p = parse_line("203.0.113.7:8080:user:pa,ss").unwrap();
        assert_eq!(p.password.as_deref(), Some("pa,ss"));

        let p = parse_line("socks5://user:pa,ss@203.0.113.7:1080").unwrap();
        assert_eq!((p.scheme.as_str(), p.password.as_deref()), ("socks5", Some("pa,ss")));
    }

    #[test]
    fn the_shapes_suppliers_actually_send() {
        let p = ok("1.2.3.4:8080");
        assert_eq!((p.scheme.as_str(), p.host.as_str(), p.port), ("http", "1.2.3.4", 8080));
        assert_eq!(p.username, None);
        assert_eq!(p.shape, Shape::HostPort);

        let p = ok("1.2.3.4:8080:bob:s3cret");
        assert_eq!(p.port, 8080);
        assert_eq!(p.username.as_deref(), Some("bob"));
        assert_eq!(p.password.as_deref(), Some("s3cret"));
        assert_eq!(p.shape, Shape::HostPortUserPass);

        let p = ok("bob:s3cret@1.2.3.4:8080");
        assert_eq!(p.username.as_deref(), Some("bob"));
        assert_eq!(p.shape, Shape::AtSign);

        let p = ok("socks5://bob:s3cret@gw.example.com:1080");
        assert_eq!(p.scheme, "socks5");
        assert_eq!(p.host, "gw.example.com");
        assert_eq!(p.shape, Shape::Url);

        // socks5h is the same thing to the relay, and saying so here keeps the
        // model from implying a choice that does not exist.
        assert_eq!(ok("socks5h://h:1080").scheme, "socks5");
    }

    #[test]
    fn a_password_may_contain_colons_and_a_username_may_not() {
        // The split has to be on the FIRST colon. Splitting on the last moves
        // part of the password into the username, and the proxy then rejects
        // every request with credentials that look almost right.
        let p = ok("http://bob:a:b:c@1.2.3.4:8080");
        assert_eq!(p.username.as_deref(), Some("bob"));
        assert_eq!(p.password.as_deref(), Some("a:b:c"));
    }

    #[test]
    fn a_reversed_line_is_refused_rather_than_reinterpreted() {
        // user:pass:host:port. Field 2 is not a port, field 4 is. Reading it
        // the other way round would send the account's traffic to a host called
        // "bob" — or, worse, to a real host that happens to share the name.
        let e = parse_line("bob:s3cret:1.2.3.4:8080").unwrap_err();
        assert!(e.to_string().contains("user:pass:host:port"), "{e}");

        // And the same line written the way Fury reads it does parse, so the
        // error above is actionable rather than a dead end.
        assert_eq!(ok("1.2.3.4:8080:bob:s3cret").port, 8080);
    }

    #[test]
    fn ipv6_needs_its_brackets() {
        let p = ok("[2001:db8::1]:1080");
        assert_eq!(p.host, "2001:db8::1");
        assert_eq!(p.port, 1080);

        let p = ok("socks5://u:p@[2001:db8::1]:1080");
        assert_eq!(p.host, "2001:db8::1");
        assert_eq!(p.username.as_deref(), Some("u"));

        // Unbracketed, it is a pile of colons with no reading a machine can be
        // sure of. Refusing is the only honest answer.
        assert!(parse_line("2001:db8::1:1080").is_err());
    }

    #[test]
    fn an_unsupported_scheme_is_an_error_not_a_downgrade() {
        let e = parse_line("socks4://1.2.3.4:1080").unwrap_err();
        assert!(e.to_string().contains("socks5"), "{e}");
        assert!(parse_line("ftp://1.2.3.4:21").is_err());
    }

    #[test]
    fn one_bad_line_does_not_take_the_block_with_it() {
        let block = "\
# supplier: acme, batch 3
1.2.3.4:8080:bob:pw

nonsense
socks5://u:p@5.6.7.8:1080
9.9.9.9:0
";
        let rows = parse_block(block);
        assert_eq!(rows.len(), 4, "comments and blanks are skipped, nothing else");
        // Line numbers are the ones in the box the operator pasted into.
        assert_eq!(rows[0].0, 2);
        assert!(rows[0].1.is_ok());
        assert_eq!(rows[1].0, 4);
        assert!(rows[1].1.is_err());
        assert_eq!(rows[2].0, 5);
        assert!(rows[2].1.is_ok());
        // Port 0 parses as a number and is still not a port.
        assert_eq!(rows[3].0, 6);
        assert!(rows[3].1.is_err());
    }

    #[test]
    fn what_comes_out_is_what_the_relay_takes_in() {
        // The whole point of the round trip: whatever shape went in, the URL
        // this produces has to be one the agent can actually dial. If these
        // two ever disagree, a bulk import saves proxies that fail at launch.
        for line in [
            "1.2.3.4:8080",
            "1.2.3.4:8080:bob:s3cret",
            "bob:s3cret@1.2.3.4:8080",
            "socks5://bob:s3cret@gw.example.com:1080",
            "https://1.2.3.4:3128",
            "[2001:db8::1]:1080",
        ] {
            let p = ok(line);
            let url = p.url();
            let reparsed = parse_line(&url)
                .unwrap_or_else(|e| panic!("{line:?} produced {url:?}, which does not parse: {e}"));
            assert_eq!(reparsed.host, p.host, "{line}");
            assert_eq!(reparsed.port, p.port, "{line}");
            assert_eq!(reparsed.scheme, p.scheme, "{line}");
            assert_eq!(reparsed.username, p.username, "{line}");
            assert_eq!(reparsed.password, p.password, "{line}");
        }
    }
}
