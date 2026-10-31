// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Farming: warming a set of profiles a little every day, for a number of days.
//!
//! One warm-up (warm.rs) fills a jar in twenty minutes, and a jar filled in
//! twenty minutes looks like it: every cookie the same age, every site visited
//! in one sitting, nothing from yesterday. A person's browser has a history
//! with days in it. Asked for by a tester (31.10.2026): "a schedule — how many
//! days to farm, how many sites to open each day".
//!
//! A plan here is: these profiles, these sites, so many days, so many sites a
//! day (a range), between these hours. Each profile gets its own day: a number
//! of sites drawn from the range, taken from the list in a fresh random order,
//! split into one to three sessions at random times inside the hours. Each
//! session is an ordinary warm-up that closes the browser when done.
//!
//! The hours are the PROFILE's hours, in its own timezone (or its proxy's, as
//! the shell resolved it when the plan was made), and this machine's only when
//! it has none. A Berlin account that reads the news at 3 a.m. Berlin time
//! because its operator lives in Novosibirsk is the odd one out.
//!
//! What it does not do, and why:
//!
//! - Catch up. A day the machine slept through is not a day; the plan moves on
//!   by one. Doing yesterday's sites at 9 a.m. today is the same jar with a
//!   stranger history.
//! - Take a profile somebody is using. An open profile is the operator's; its
//!   session waits half an hour and tries again, and gives up when the hours
//!   run out.
//! - Open a team profile itself. That takes the server's lock, the bundle and
//!   the organisation key, which live in the shell. A team session that is due
//!   waits for the shell to claim it (`farm.due` / `farm.claim`), so team
//!   profiles farm only while Fury is open. A local one farms whenever the
//!   agent runs, window or none.
//! - Run more than `at_once` browsers for one plan. Twenty Chromes started at
//!   ten o'clock is a frozen laptop, and twenty profiles all active in the same
//!   minute is a pattern of its own.
//!
//! State is one JSON file beside the database, rewritten when it changes. A run
//! that was in flight when the agent stopped did not report and never will;
//! loading marks it failed rather than leaving it to hold a slot.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::warm;

/// More than this is a different product.
pub const MAX_DAYS: u32 = 90;
const MAX_SITES_PER_DAY: u32 = 100;
const MAX_AT_ONCE: u32 = 6;
/// A run that has not reported in this long is not going to.
const STALE_MS: u64 = 2 * 60 * 60 * 1000;
/// A day with less of its hours left than this is not started.
const MIN_WINDOW_MS: i64 = 10 * 60 * 1000;

/// One profile in a plan, as the shell described it when the plan was made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub name: String,
    /// "local", "team" or "shared", as the shell has it. Anything but local is
    /// opened by the shell.
    #[serde(default = "local")]
    pub origin: String,
    /// IANA name the profile's clock follows; none means this machine's.
    #[serde(default)]
    pub timezone: Option<String>,
}

fn local() -> String {
    "local".into()
}

impl Target {
    pub fn is_local(&self) -> bool {
        self.origin == "local"
    }
}

/// What the operator asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub name: String,
    pub urls: Vec<String>,
    pub days: u32,
    /// Sites a day, as a range; each profile draws its own number each day.
    pub sites_per_day: (u32, u32),
    /// Local hours, [from, to). 0 and 24 are the day's ends.
    #[serde(default = "default_hours")]
    pub hours: (u32, u32),
    #[serde(default = "default_dwell")]
    pub dwell_seconds: (u64, u64),
    #[serde(default)]
    pub follow_link: bool,
    /// Browsers this plan may have open at once.
    #[serde(default = "default_at_once")]
    pub at_once: u32,
    /// Set the stage to "ready" when a profile's last day is done. The shell
    /// applies it, because a team profile's stage lives on the server.
    #[serde(default)]
    pub set_stage: bool,
}

fn default_hours() -> (u32, u32) {
    (10, 22)
}
fn default_dwell() -> (u64, u64) {
    (8, 25)
}
fn default_at_once() -> u32 {
    2
}

impl Spec {
    pub fn check(&self) -> anyhow::Result<()> {
        if self.urls.iter().all(|u| u.trim().is_empty()) {
            anyhow::bail!("nothing to visit: the list is empty");
        }
        if !(1..=MAX_DAYS).contains(&self.days) {
            anyhow::bail!("days: from 1 to {MAX_DAYS}");
        }
        let (lo, hi) = self.sites_per_day;
        if lo < 1 || hi < lo || hi > MAX_SITES_PER_DAY {
            anyhow::bail!("sites a day: from 1 to {MAX_SITES_PER_DAY}, the first no more than the second");
        }
        let (from, to) = self.hours;
        if to > 24 || from >= to {
            anyhow::bail!("hours: from 0 to 24, the start before the end");
        }
        if !(1..=MAX_AT_ONCE).contains(&self.at_once) {
            anyhow::bail!("at once: from 1 to {MAX_AT_ONCE}");
        }
        let (dlo, dhi) = self.dwell_seconds;
        if dlo < 1 || dhi < dlo {
            anyhow::bail!("time per page: the first no more than the second");
        }
        Ok(())
    }
}

/// How a session ended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Done { sites: usize, cookies: usize },
    Failed { error: String },
    Skipped { why: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub at_ms: u64,
    pub urls: Vec<String>,
    #[serde(default)]
    pub started_ms: Option<u64>,
    #[serde(default)]
    pub ended: Option<Outcome>,
}

impl Session {
    fn in_flight(&self) -> bool {
        self.started_ms.is_some() && self.ended.is_none()
    }
    fn waiting(&self) -> bool {
        self.started_ms.is_none() && self.ended.is_none()
    }
}

/// One profile's way through a plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    pub days_done: u32,
    /// The last local day planned, "YYYY-MM-DD". The next starts after it.
    #[serde(default)]
    pub last_day: Option<String>,
    /// The day `sessions` belong to, while one is open.
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default)]
    pub window_end_ms: u64,
    #[serde(default)]
    pub sessions: Vec<Session>,
    /// Sites visited over the whole plan, and the jar at the last session.
    #[serde(default)]
    pub sites: u64,
    #[serde(default)]
    pub cookies: Option<usize>,
    /// The last thing that went wrong, for the operator.
    #[serde(default)]
    pub last_error: Option<String>,
    /// A stage the shell is to set, then acknowledge.
    #[serde(default)]
    pub stage_pending: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Farm {
    pub id: String,
    pub created_ms: u64,
    pub spec: Spec,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub tracks: BTreeMap<String, Track>,
    #[serde(default)]
    pub paused: bool,
}

impl Farm {
    fn in_flight(&self) -> usize {
        self.tracks.values().flat_map(|t| &t.sessions).filter(|s| s.in_flight()).count()
    }
}

/// A local session the agent is to start now.
#[derive(Debug, Clone)]
pub struct Start {
    pub profile_id: String,
    pub name: String,
    pub plan: warm::Plan,
}

/// A team session that is due, for the shell to claim.
#[derive(Debug, Clone, Serialize)]
pub struct Due {
    pub farm_id: String,
    pub profile_id: String,
    pub name: String,
    pub origin: String,
}

pub struct Farms {
    path: PathBuf,
    pub list: Vec<Farm>,
    written: String,
}

impl Farms {
    /// Read the file, and fail every run that was in flight: it was in the
    /// agent that is no longer running.
    pub fn load(path: PathBuf) -> Self {
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let mut list: Vec<Farm> = serde_json::from_str(&raw).unwrap_or_default();
        for farm in &mut list {
            for track in farm.tracks.values_mut() {
                for s in track.sessions.iter_mut().filter(|s| s.in_flight()) {
                    s.ended = Some(Outcome::Failed { error: "Fury stopped during the session".into() });
                }
            }
        }
        let mut me = Self { path, list, written: raw };
        me.save();
        me
    }

    /// Write if anything changed. Beside and renamed over, like templates.
    pub fn save(&mut self) {
        let Ok(body) = serde_json::to_string_pretty(&self.list) else { return };
        if body == self.written {
            return;
        }
        let tmp = self.path.with_extension("json.tmp");
        match std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => self.written = body,
            Err(e) => tracing::warn!(error = %e, "could not save the farming plans"),
        }
    }

    pub fn create(&mut self, spec: Spec, targets: Vec<Target>, now_ms: u64) -> anyhow::Result<String> {
        spec.check()?;
        if targets.is_empty() {
            anyhow::bail!("no profiles in the plan");
        }
        // One plan per profile: two plans warming the same account would be
        // two schedules on one person.
        for t in &targets {
            if let Some(f) = self.list.iter().find(|f| !f.finished() && f.targets.iter().any(|x| x.id == t.id)) {
                anyhow::bail!("{} is already in the plan \"{}\"", t.name, f.label());
            }
        }
        let id = uuid::Uuid::now_v7().to_string();
        let spec = Spec { urls: spec.urls.iter().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect(), ..spec };
        self.list.push(Farm { id: id.clone(), created_ms: now_ms, spec, targets, tracks: BTreeMap::new(), paused: false });
        self.save();
        Ok(id)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|f| f.id != id);
        self.save();
        self.list.len() != before
    }

    pub fn pause(&mut self, id: &str, paused: bool) -> bool {
        let found = self.list.iter_mut().find(|f| f.id == id).map(|f| f.paused = paused).is_some();
        self.save();
        found
    }

    /// The run of a session ended. Called from the warm-up's completion.
    pub fn report(&mut self, farm_id: &str, profile_id: &str, last: &warm::Progress) {
        let Some(track) = self.track_mut(farm_id, profile_id) else { return };
        let Some(s) = track.sessions.iter_mut().find(|s| s.in_flight()) else { return };
        s.ended = Some(match (&last.error, last.done) {
            (Some(e), 0) => Outcome::Failed { error: e.clone() },
            (None, 0) if last.stopped => Outcome::Failed { error: "stopped".into() },
            _ => Outcome::Done { sites: last.done, cookies: last.cookies_now },
        });
        track.sites += last.done as u64;
        if last.cookies_now > 0 {
            track.cookies = Some(last.cookies_now);
        }
        track.last_error = last.error.clone();
        self.save();
    }

    /// The shell could not start a session it claimed.
    pub fn fail(&mut self, farm_id: &str, profile_id: &str, error: &str) {
        let Some(track) = self.track_mut(farm_id, profile_id) else { return };
        if let Some(s) = track.sessions.iter_mut().find(|s| s.in_flight()) {
            s.ended = Some(Outcome::Failed { error: error.to_string() });
        }
        track.last_error = Some(error.to_string());
        self.save();
    }

    pub fn stage_applied(&mut self, farm_id: &str, profile_id: &str) {
        if let Some(track) = self.track_mut(farm_id, profile_id) {
            track.stage_pending = None;
        }
        self.save();
    }

    fn track_mut(&mut self, farm_id: &str, profile_id: &str) -> Option<&mut Track> {
        self.list.iter_mut().find(|f| f.id == farm_id)?.tracks.get_mut(profile_id)
    }

    /// Team sessions that are due and that their plan has room for.
    pub fn due(&self, now: DateTime<Utc>, open: &HashSet<String>) -> Vec<Due> {
        let now_ms = ms(now);
        let mut out = Vec::new();
        for farm in self.list.iter().filter(|f| !f.paused) {
            let mut room = (farm.spec.at_once as usize).saturating_sub(farm.in_flight());
            for t in farm.targets.iter().filter(|t| !t.is_local()) {
                if room == 0 {
                    break;
                }
                let Some(track) = farm.tracks.get(&t.id) else { continue };
                let due = track.sessions.iter().find(|s| s.waiting()).is_some_and(|s| s.at_ms <= now_ms);
                if due && !open.contains(&t.id) && !track.sessions.iter().any(|s| s.in_flight()) {
                    out.push(Due { farm_id: farm.id.clone(), profile_id: t.id.clone(), name: t.name.clone(), origin: t.origin.clone() });
                    room -= 1;
                }
            }
        }
        out
    }

    /// The shell takes a due team session: it is marked started and its plan
    /// handed over. Refused if it is no longer due, so two windows cannot both
    /// start it.
    pub fn claim(&mut self, farm_id: &str, profile_id: &str, now: DateTime<Utc>, open: &HashSet<String>) -> anyhow::Result<warm::Plan> {
        if !self.due(now, open).iter().any(|d| d.farm_id == farm_id && d.profile_id == profile_id) {
            anyhow::bail!("that session is not due");
        }
        let farm = self.list.iter_mut().find(|f| f.id == farm_id).expect("due found it");
        let spec = farm.spec.clone();
        let track = farm.tracks.get_mut(profile_id).expect("due found it");
        let s = track.sessions.iter_mut().find(|s| s.waiting()).expect("due found it");
        s.started_ms = Some(ms(now));
        let plan = session_plan(&spec, farm_id, &s.urls);
        self.save();
        Ok(plan)
    }

    /// One pass over every plan: close days that are over, plan the next,
    /// move sessions that are due. Returns the local sessions to start now.
    pub fn step(&mut self, now: DateTime<Utc>, open: &HashSet<String>, rng: &mut impl Rng) -> Vec<Start> {
        let now_ms = ms(now);
        let mut starts = Vec::new();
        for farm in self.list.iter_mut().filter(|f| !f.paused) {
            let mut room = (farm.spec.at_once as usize).saturating_sub(farm.in_flight());
            for t in &farm.targets {
                let track = farm.tracks.entry(t.id.clone()).or_default();
                if track.days_done >= farm.spec.days {
                    continue;
                }
                if let Some(s) = track.sessions.iter_mut().find(|s| s.in_flight()) {
                    if now_ms.saturating_sub(s.started_ms.unwrap_or(now_ms)) > STALE_MS {
                        s.ended = Some(Outcome::Failed { error: "the session never reported back".into() });
                        room += 1;
                    }
                    continue;
                }

                // Close the day: every session over, or the hours are.
                if track.day.is_some() && (track.sessions.iter().all(|s| s.ended.is_some()) || now_ms >= track.window_end_ms) {
                    for s in track.sessions.iter_mut().filter(|s| s.waiting()) {
                        s.ended = Some(Outcome::Skipped { why: "the day's hours ran out".into() });
                    }
                    // A day counts if anything was read in it. Nothing read
                    // means the plan moves on a day; it does not catch up.
                    if track.sessions.iter().any(|s| matches!(s.ended, Some(Outcome::Done { .. }))) {
                        track.days_done += 1;
                    }
                    track.day = None;
                    if track.days_done >= farm.spec.days {
                        if farm.spec.set_stage {
                            track.stage_pending = Some("ready".into());
                        }
                        continue;
                    }
                }

                if track.day.is_none() {
                    let zone = Zone::of(t.timezone.as_deref());
                    let after = track.last_day.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
                    let (day, end, sessions) = plan_day(&farm.spec, &zone, after, now, rng);
                    track.day = Some(day.format("%Y-%m-%d").to_string());
                    track.last_day = track.day.clone();
                    track.window_end_ms = end;
                    track.sessions = sessions;
                }

                let Some(s) = track.sessions.iter_mut().find(|s| s.waiting()) else { continue };
                if s.at_ms > now_ms {
                    continue;
                }
                if open.contains(&t.id) {
                    // Somebody is using it. Try again later in the day.
                    s.at_ms = now_ms + rng.gen_range(20..=40) * 60 * 1000;
                    if s.at_ms >= track.window_end_ms {
                        s.ended = Some(Outcome::Skipped { why: "the profile was open".into() });
                    }
                    continue;
                }
                if !t.is_local() || room == 0 {
                    // The shell claims a team one; a local one waits its turn.
                    continue;
                }
                s.started_ms = Some(now_ms);
                room -= 1;
                starts.push(Start { profile_id: t.id.clone(), name: t.name.clone(), plan: session_plan(&farm.spec, &farm.id, &s.urls) });
            }
        }
        self.save();
        starts
    }
}

impl Farm {
    pub fn finished(&self) -> bool {
        self.targets.iter().all(|t| self.tracks.get(&t.id).is_some_and(|tr| tr.days_done >= self.spec.days))
    }

    fn label(&self) -> String {
        if self.spec.name.trim().is_empty() {
            format!("{} profiles", self.targets.len())
        } else {
            self.spec.name.trim().to_string()
        }
    }
}

fn session_plan(spec: &Spec, farm_id: &str, urls: &[String]) -> warm::Plan {
    warm::Plan {
        urls: urls.to_vec(),
        dwell_seconds: spec.dwell_seconds,
        follow_link: spec.follow_link,
        // The plan opened it, so the plan closes it: nobody is watching at
        // 16:40 to close twenty windows.
        close_after: true,
        farm_id: Some(farm_id.to_string()),
    }
}

/// A profile's clock: a named zone, or this machine's.
pub enum Zone {
    Named(chrono_tz::Tz),
    Host,
}

impl Zone {
    pub fn of(name: Option<&str>) -> Self {
        match name.and_then(|n| n.trim().parse::<chrono_tz::Tz>().ok()) {
            Some(tz) => Zone::Named(tz),
            None => Zone::Host,
        }
    }

    fn today(&self, now: DateTime<Utc>) -> NaiveDate {
        match self {
            Zone::Named(tz) => now.with_timezone(tz).date_naive(),
            Zone::Host => now.with_timezone(&chrono::Local).date_naive(),
        }
    }

    /// The instant `hour` o'clock on `date` happens here. 24 is the next
    /// midnight; an hour a DST change skips is the one after it.
    fn at(&self, date: NaiveDate, hour: u32) -> DateTime<Utc> {
        let (date, hour) = if hour >= 24 { (date.succ_opt().unwrap_or(date), 0) } else { (date, hour) };
        let naive = date.and_hms_opt(hour, 0, 0).expect("hour is under 24");
        let pick = |n: chrono::NaiveDateTime| -> Option<DateTime<Utc>> {
            match self {
                Zone::Named(tz) => tz.from_local_datetime(&n).earliest().map(|d| d.with_timezone(&Utc)),
                Zone::Host => chrono::Local.from_local_datetime(&n).earliest().map(|d| d.with_timezone(&Utc)),
            }
        };
        pick(naive)
            .or_else(|| pick(naive + Duration::hours(1)))
            .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
    }
}

/// The next day to farm, after `after` and not before today, whose hours have
/// enough left in them; its end, and its sessions.
pub fn plan_day(spec: &Spec, zone: &Zone, after: Option<NaiveDate>, now: DateTime<Utc>, rng: &mut impl Rng) -> (NaiveDate, u64, Vec<Session>) {
    let mut date = zone.today(now);
    if let Some(last) = after {
        if date <= last {
            date = last.succ_opt().unwrap_or(last);
        }
    }
    let (begin, end) = loop {
        let start = zone.at(date, spec.hours.0);
        let end = zone.at(date, spec.hours.1);
        let begin = start.max(now + Duration::minutes(1));
        if (end - begin).num_milliseconds() >= MIN_WINDOW_MS {
            break (begin, end);
        }
        date = date.succ_opt().unwrap_or(date);
    };

    let (lo, hi) = spec.sites_per_day;
    let k = (rng.gen_range(lo..=hi) as usize).clamp(1, spec.urls.len().max(1));
    let mut urls = spec.urls.clone();
    urls.shuffle(rng);
    urls.truncate(k);

    // One sitting for a handful of sites; for more, two or three spread over
    // the hours, the way a person comes back to the browser.
    let n = if k <= 3 { 1 } else { rng.gen_range(1..=(k / 2).min(3)) };
    let span = (end - begin).num_milliseconds().max(1) as u64;
    let slot = span / n as u64;
    let mut sessions = Vec::with_capacity(n);
    let mut rest = urls.as_slice();
    for i in 0..n {
        let take = rest.len() / (n - i);
        let (mine, after) = rest.split_at(take);
        rest = after;
        // Early in each slot rather than anywhere in it, so a long session
        // still ends before the next one is due.
        let at = ms(begin) + slot * i as u64 + rng.gen_range(0..=slot * 6 / 10);
        sessions.push(Session { at_ms: at, urls: mine.to_vec(), started_ms: None, ended: None });
    }
    (date, ms(end), sessions)
}

fn ms(t: DateTime<Utc>) -> u64 {
    t.timestamp_millis().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn spec() -> Spec {
        Spec {
            name: "t".into(),
            urls: (0..20).map(|i| format!("https://s{i}.example/")).collect(),
            days: 3,
            sites_per_day: (5, 8),
            hours: (10, 22),
            dwell_seconds: (8, 25),
            follow_link: false,
            at_once: 2,
            set_stage: true,
        }
    }

    fn target(id: &str, origin: &str) -> Target {
        Target { id: id.into(), name: id.into(), origin: origin.into(), timezone: Some("Europe/Berlin".into()) }
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn farms(targets: Vec<Target>) -> Farms {
        let dir = std::env::temp_dir().join(format!("fury-farm-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Farms::load(dir.join("farms.json"));
        f.create(spec(), targets, 0).unwrap();
        f
    }

    #[test]
    fn a_day_falls_inside_the_profiles_hours() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let zone = Zone::of(Some("Europe/Berlin"));
        // 05:00 UTC is 06:00 in Berlin in winter: before the hours.
        let now = utc("2026-11-10T05:00:00Z");
        for _ in 0..200 {
            let (day, end, sessions) = plan_day(&spec(), &zone, None, now, &mut rng);
            assert_eq!(day.to_string(), "2026-11-10");
            assert_eq!(end, ms(utc("2026-11-10T21:00:00Z")));
            let sites: usize = sessions.iter().map(|s| s.urls.len()).sum();
            assert!((5..=8).contains(&sites), "{sites}");
            assert!((1..=3).contains(&sessions.len()));
            for s in &sessions {
                assert!(s.at_ms >= ms(utc("2026-11-10T09:00:00Z")) && s.at_ms < end);
                assert!(!s.urls.is_empty());
            }
            assert!(sessions.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
        }
    }

    #[test]
    fn a_day_whose_hours_are_over_starts_tomorrow() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let zone = Zone::of(Some("Asia/Novosibirsk"));
        // 16:30 UTC is 23:30 in Novosibirsk.
        let (day, _, sessions) = plan_day(&spec(), &zone, None, utc("2026-11-10T16:30:00Z"), &mut rng);
        assert_eq!(day.to_string(), "2026-11-11");
        assert!(sessions[0].at_ms >= ms(utc("2026-11-11T03:00:00Z")));
    }

    #[test]
    fn the_next_day_comes_after_the_last_one() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let zone = Zone::of(Some("Europe/Berlin"));
        let last = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let (day, _, _) = plan_day(&spec(), &zone, Some(last), utc("2026-11-10T12:00:00Z"), &mut rng);
        assert_eq!(day.to_string(), "2026-11-11");
    }

    #[test]
    fn a_short_list_is_all_visited_and_no_more() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(4);
        let s = Spec { urls: vec!["a.example".into(), "b.example".into()], ..spec() };
        let (_, _, sessions) = plan_day(&s, &Zone::of(Some("UTC")), None, utc("2026-11-10T05:00:00Z"), &mut rng);
        assert_eq!(sessions.iter().map(|s| s.urls.len()).sum::<usize>(), 2);
    }

    #[test]
    fn local_sessions_start_no_more_than_at_once() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(5);
        let mut f = farms(vec![target("a", "local"), target("b", "local"), target("c", "local")]);
        let open = HashSet::new();
        f.step(utc("2026-11-10T05:00:00Z"), &open, &mut rng);
        // Late in the hours, everything is due.
        let starts = f.step(utc("2026-11-10T20:59:00Z"), &open, &mut rng);
        assert_eq!(starts.len(), 2);
        assert!(starts.iter().all(|s| s.plan.close_after && s.plan.farm_id.is_some()));
        assert!(f.step(utc("2026-11-10T20:59:10Z"), &open, &mut rng).is_empty());
    }

    #[test]
    fn a_team_session_waits_for_the_shell_and_is_claimed_once() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(6);
        let mut f = farms(vec![target("t", "team")]);
        let open = HashSet::new();
        f.step(utc("2026-11-10T05:00:00Z"), &open, &mut rng);
        let now = utc("2026-11-10T20:59:00Z");
        assert!(f.step(now, &open, &mut rng).is_empty());
        let due = f.due(now, &open);
        assert_eq!(due.len(), 1);
        let fid = due[0].farm_id.clone();
        let plan = f.claim(&fid, "t", now, &open).unwrap();
        assert_eq!(plan.farm_id.as_deref(), Some(fid.as_str()));
        assert!(f.claim(&fid, "t", now, &open).is_err());
        assert!(f.due(now, &open).is_empty());
    }

    #[test]
    fn an_open_profile_is_left_alone() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mut f = farms(vec![target("a", "local")]);
        let open: HashSet<String> = ["a".to_string()].into();
        f.step(utc("2026-11-10T05:00:00Z"), &open, &mut rng);
        assert!(f.step(utc("2026-11-10T20:59:00Z"), &open, &mut rng).is_empty());
        let track = &f.list[0].tracks["a"];
        assert!(track.sessions.iter().all(|s| s.started_ms.is_none()));
    }

    fn done(n: usize) -> warm::Progress {
        warm::Progress {
            profile_id: "a".into(),
            name: "a".into(),
            total: n,
            done: n,
            current: None,
            cookies_before: 0,
            cookies_now: 40,
            finished: true,
            stopped: false,
            error: None,
            started_at_ms: 0,
        }
    }

    /// Run a local profile through the plan in ten-minute steps, reporting
    /// every session done.
    #[test]
    fn days_are_counted_and_the_stage_is_asked_for_at_the_end() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(8);
        let mut f = farms(vec![target("a", "local")]);
        let fid = f.list[0].id.clone();
        let open = HashSet::new();
        let mut t = utc("2026-11-10T05:00:00Z");
        for _ in 0..(5 * 24 * 60 / 10) {
            for s in f.step(t, &open, &mut rng) {
                f.report(&fid, &s.profile_id, &done(s.plan.urls.len()));
            }
            t += Duration::minutes(10);
        }
        let track = &f.list[0].tracks["a"];
        assert_eq!(track.days_done, 3);
        assert_eq!(track.last_day.as_deref(), Some("2026-11-12"));
        assert_eq!(track.stage_pending.as_deref(), Some("ready"));
        assert!(track.sites >= 15);
        assert!(f.list[0].finished());
    }

    #[test]
    fn a_day_with_nothing_read_does_not_count() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(9);
        let mut f = farms(vec![target("a", "local")]);
        let fid = f.list[0].id.clone();
        let open = HashSet::new();
        let mut t = utc("2026-11-10T05:00:00Z");
        // Every session fails on the first day.
        while t < utc("2026-11-10T23:00:00Z") {
            for s in f.step(t, &open, &mut rng) {
                f.fail(&fid, &s.profile_id, "no proxy");
            }
            t += Duration::minutes(10);
        }
        f.step(t, &open, &mut rng);
        let track = &f.list[0].tracks["a"];
        assert_eq!(track.days_done, 0);
        assert_eq!(track.day.as_deref(), Some("2026-11-11"));
        assert_eq!(track.last_error.as_deref(), Some("no proxy"));
    }

    #[test]
    fn a_run_in_flight_when_the_agent_stopped_is_failed_on_load() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(10);
        let mut f = farms(vec![target("a", "local")]);
        let open = HashSet::new();
        f.step(utc("2026-11-10T05:00:00Z"), &open, &mut rng);
        assert_eq!(f.step(utc("2026-11-10T20:59:00Z"), &open, &mut rng).len(), 1);
        let again = Farms::load(f.path.clone());
        let s = &again.list[0].tracks["a"].sessions[0];
        assert!(matches!(s.ended, Some(Outcome::Failed { .. })));
    }

    #[test]
    fn one_profile_is_in_one_plan() {
        let mut f = farms(vec![target("a", "local")]);
        assert!(f.create(spec(), vec![target("a", "local")], 0).is_err());
        assert!(f.create(spec(), vec![target("b", "local")], 0).is_ok());
    }

    #[test]
    fn a_bad_plan_is_refused() {
        assert!(Spec { days: 0, ..spec() }.check().is_err());
        assert!(Spec { sites_per_day: (5, 2), ..spec() }.check().is_err());
        assert!(Spec { hours: (22, 10), ..spec() }.check().is_err());
        assert!(Spec { urls: vec![" ".into()], ..spec() }.check().is_err());
        assert!(spec().check().is_ok());
    }
}
