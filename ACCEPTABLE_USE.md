# Acceptable use

Fury is a browser privacy and multi-account management tool. Like a VPN or a
password manager, it is dual-use: the same capability that protects a legitimate
operator protects a malicious one. This document sets the boundaries of what
this project supports, what it will help with, and what it will not.

It is not a licence restriction — the code is AGPL and you may run it however
you like. It defines the scope of the project: what gets built, what gets
answered in the issue tracker, and who this community is for.

## Built for

- Managing multiple legitimate business accounts — marketplace seller accounts,
  advertising cabinets, client accounts at an agency
- Ad verification and competitive research
- QA and cross-environment testing
- Web scraping within a site's terms and applicable law
- Journalists, researchers and privacy-conscious users resisting cross-site
  tracking and fingerprinting

## Not built for

- Fraud of any kind: payment fraud, chargeback abuse, fake reviews, ad fraud
- Credential stuffing, account takeover, mass automated registration
- Phishing, impersonation, or any social engineering infrastructure
- Evading law enforcement, sanctions, or court orders
- Harassment, stalking, or targeting private individuals
- Bypassing age verification or identity verification required by law

## What this means in the issue tracker

Requests to defeat a specific platform's identity or fraud checks will be closed.
"How do I make N accounts on <platform> that survive their verification" is not
a supported use case, regardless of how it is phrased.

Reports of the form "fingerprint vector X leaks in context Y" are exactly what
this project wants, and are welcome without qualification.

## What the project deliberately does not ship

- Ready-made automation scripts targeting specific platforms
- Bundled proxy services
- Any claim of defeating a named anti-fraud vendor

The local automation API is a general tool. It is not, and will not become, a
bot for any particular website.

## An honest limit

The "operator cannot export data" permission model is designed against a
dishonest employee without special skills. It is not designed against a
motivated reverse engineer with administrator rights on their own machine —
that person can dump process memory. Anyone deploying Fury for a team should
understand this rather than treat the restriction as absolute.
