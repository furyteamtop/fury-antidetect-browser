// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Account recovery and owner hand-over, through the router, against a real
//! PostgreSQL (recovery.rs, docs/25).
//!
//! Through the router because what matters is what a client can do with the
//! HTTP surface, not whether a function returns the right value when called
//! correctly. Same database and the same lock as rls_tests, same skip rule:
//!
//! ```bash
//! DATABASE_URL=postgres://app@127.0.0.1:55432/apptest cargo test -p fury-server recovery
//! ```
//!
//! No Argon2 on the client side here: the server never sees the code, only the
//! "auth" half, so the tests send random 32-byte values in its place. The
//! private key is a real X25519 key, because the challenge is opened with it.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::rls_tests::{fixture, DB_LOCK, ORG_A, USER_A};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const ADMIN: &str = "cccccccc-0000-0000-0000-000000000003";
const MEMBER: &str = "dddddddd-0000-0000-0000-000000000004";
const PASSWORD: &str = "the old password";

struct World {
    app: axum::Router,
    pool: sqlx::PgPool,
    owner_secret: [u8; 32],
}

async fn world() -> Option<World> {
    let mut c = fixture().await?;
    let owner = fury_shared::keys::new_keypair();
    let hash = crate::auth::hash_password(PASSWORD).unwrap();
    // The seeded owner of A gets a real password and a real public key; an
    // admin who holds the organisation key and a member join them.
    sqlx::query("UPDATE users SET password_hash = $2, public_key = $3, wrapped_private_key = $4, kdf_salt = $5 WHERE id = $1::uuid")
        .bind(USER_A)
        .bind(&hash)
        .bind(owner.public.to_vec())
        .bind(vec![7u8; 72])
        .bind(vec![1u8; 16])
        .execute(&mut c)
        .await
        .unwrap();
    for (id, email, role, ork) in [(ADMIN, "admin@example.com", "admin", Some(vec![1u8])), (MEMBER, "m@example.com", "member", Some(vec![1u8]))] {
        sqlx::query("INSERT INTO users (id, email, password_hash, public_key, wrapped_private_key, kdf_salt) VALUES ($1::uuid, $2, $3, '\\x00', '\\x00', '\\x00')")
            .bind(id).bind(email).bind(&hash).execute(&mut c).await.unwrap();
        sqlx::query("INSERT INTO org_members (org_id, user_id, role, wrapped_ork, ork_generation) VALUES ($1::uuid, $2::uuid, $3::org_role, $4, 1)")
            .bind(ORG_A).bind(id).bind(role).bind(ork).execute(&mut c).await.unwrap();
    }
    let pool = crate::connect().await.unwrap();
    let state = Arc::new(crate::AppState { db: pool.clone(), open_signup: false, limits: Default::default(), mailer: None });
    let app = axum::Router::new().merge(crate::api::routes()).with_state(state);
    Some(World { app, pool, owner_secret: owner.secret })
}

async fn call(w: &World, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().uri(path).method(if body.is_some() { "POST" } else { "GET" });
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = w.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn login(w: &World, email: &str, password: &str) -> Option<String> {
    let (s, v) = call(w, "/v1/auth/login", None, Some(json!({ "email": email, "password": password, "machine_name": "test" }))).await;
    (s == StatusCode::OK).then(|| v["token"].as_str().unwrap().to_string())
}

fn material() -> (Value, String) {
    let auth = B64.encode(fury_shared::keys::new_org_key());
    (json!({ "salt": B64.encode([3u8; 16]), "wrapped_key": B64.encode([5u8; 72]), "auth": auth.clone() }), auth)
}

fn reason(v: &Value) -> &str {
    v["reason"].as_str().unwrap_or("")
}

#[tokio::test]
async fn recovery_end_to_end_with_the_wait() {
    let _g = DB_LOCK.lock().await;
    let Some(w) = world().await else { return eprintln!("SKIPPED: DATABASE_URL unset") };
    let owner = login(&w, "a@example.com", PASSWORD).await.expect("owner signs in");

    // Making a key wants the password again.
    let (m, auth) = material();
    let mut wrong = m.clone();
    wrong["password"] = json!("not it");
    let (s, v) = call(&w, "/v1/me/recovery", Some(&owner), Some(wrong)).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "wrong_password"));
    let mut right = m.clone();
    right["password"] = json!(PASSWORD);
    let (s, _) = call(&w, "/v1/me/recovery", Some(&owner), Some(right)).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&w, "/v1/me/recovery", Some(&owner), None).await;
    assert_eq!(v["has_key"], true);

    // A wrong key, and an unknown address, are one answer.
    let start = |auth: &str, email: &str| json!({ "email": email, "auth": auth, "machine_name": "laptop" });
    let other = B64.encode([9u8; 32]);
    let (s1, _) = call(&w, "/v1/auth/recover/start", None, Some(start(&other, "a@example.com"))).await;
    let (s2, _) = call(&w, "/v1/auth/recover/start", None, Some(start(&auth, "nobody@example.com"))).await;
    assert_eq!((s1, s2), (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));

    // The right key: no TOTP, no mail, so the wait.
    let (s, v) = call(&w, "/v1/auth/recover/start", None, Some(start(&auth, "a@example.com"))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["factor"], "wait");
    let handle = json!({ "request": v["request"], "secret": v["secret"] });
    let (s, v) = call(&w, "/v1/auth/recover/material", None, Some(handle.clone())).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "recovery_not_ready"), "nothing before the wait ends");

    // The admin sees it and cancels it; the attempt is gone.
    let admin = login(&w, "admin@example.com", PASSWORD).await.unwrap();
    let (_, pending) = call(&w, "/v1/org/recovery", Some(&admin), None).await;
    assert_eq!(pending.as_array().unwrap().len(), 1, "{pending}");
    assert_eq!(pending[0]["email"], "a@example.com");
    let member = login(&w, "m@example.com", PASSWORD).await.unwrap();
    let (_, seen) = call(&w, "/v1/org/recovery", Some(&member), None).await;
    assert_eq!(seen, json!([]), "a member is not shown the team's recoveries");
    let id = pending[0]["id"].as_str().unwrap().to_string();
    let (s, _) = call(&w, &format!("/v1/recovery/{id}/cancel"), Some(&member), Some(json!({}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nor can a member cancel one");
    let (s, _) = call(&w, &format!("/v1/recovery/{id}/cancel"), Some(&admin), Some(json!({}))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = call(&w, "/v1/auth/recover/status", None, Some(handle.clone())).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "recovery_closed"));

    // Again, and this time the 24 hours pass.
    let (_, v) = call(&w, "/v1/auth/recover/start", None, Some(start(&auth, "a@example.com"))).await;
    let handle = json!({ "request": v["request"], "secret": v["secret"] });
    sqlx::query("UPDATE recovery_requests SET ready_at = now() - interval '1 second' WHERE id = $1::uuid")
        .bind(v["request"].as_str().unwrap())
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, mat) = call(&w, "/v1/auth/recover/material", None, Some(handle.clone())).await;
    assert_eq!(s, StatusCode::OK, "{mat}");
    let sealed = B64.decode(mat["challenge"].as_str().unwrap()).unwrap();
    let nonce = fury_shared::keys::open_sealed(&w.owner_secret, &sealed).expect("sealed to the owner's key");

    let (next, next_auth) = material();
    let finish = |answer: Vec<u8>| {
        let mut f = handle.clone();
        f["answer"] = json!(B64.encode(answer));
        f["password"] = json!("a brand new password");
        f["kdf_salt"] = json!(B64.encode([4u8; 16]));
        f["wrapped_private_key"] = json!(B64.encode([6u8; 72]));
        f["recovery"] = next.clone();
        f["machine_name"] = json!("laptop");
        f
    };
    let (s, v) = call(&w, "/v1/auth/recover/finish", None, Some(finish(vec![0u8; 32]))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "wrong_answer"), "the private key has to be shown");
    let (s, v) = call(&w, "/v1/auth/recover/finish", None, Some(finish(nonce.clone()))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["token"].is_string(), "a session comes back");
    let (s, _) = call(&w, "/v1/auth/recover/finish", None, Some(finish(nonce))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "an attempt finishes once");

    // The old password and the old session are gone; the new password works;
    // the spent key opens nothing, the new one starts an attempt.
    let (s, _) = call(&w, "/v1/me/recovery", Some(&owner), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "every old session is revoked");
    assert!(login(&w, "a@example.com", PASSWORD).await.is_none());
    assert!(login(&w, "a@example.com", "a brand new password").await.is_some());
    let (s, _) = call(&w, "/v1/auth/recover/start", None, Some(start(&auth, "a@example.com"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the spent key ended with the recovery");
    let (s, _) = call(&w, "/v1/auth/recover/start", None, Some(start(&next_auth, "a@example.com"))).await;
    assert_eq!(s, StatusCode::OK, "the key made at the end works");
}

#[tokio::test]
async fn five_wrong_keys_close_the_door_for_the_hour() {
    let _g = DB_LOCK.lock().await;
    let Some(w) = world().await else { return eprintln!("SKIPPED: DATABASE_URL unset") };
    let owner = login(&w, "a@example.com", PASSWORD).await.unwrap();
    let (mut m, auth) = material();
    m["password"] = json!(PASSWORD);
    call(&w, "/v1/me/recovery", Some(&owner), Some(m)).await;
    for _ in 0..5 {
        let (s, _) = call(&w, "/v1/auth/recover/start", None, Some(json!({ "email": "a@example.com", "auth": B64.encode([9u8; 32]) }))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, v) = call(&w, "/v1/auth/recover/start", None, Some(json!({ "email": "a@example.com", "auth": auth }))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "too_many_attempts"), "not even the right key, for the hour");
}

#[tokio::test]
async fn with_an_authenticator_the_code_is_the_second_step() {
    let _g = DB_LOCK.lock().await;
    let Some(w) = world().await else { return eprintln!("SKIPPED: DATABASE_URL unset") };
    let secret = vec![42u8; 20];
    sqlx::query("UPDATE users SET totp_secret = $2, totp_enabled_at = now() WHERE id = $1::uuid")
        .bind(USER_A).bind(&secret).execute(&w.pool).await.unwrap();
    let code = || {
        let t = fury_shared::totp::Totp { secret: secret.clone(), digits: 6, period: 30, algorithm: fury_shared::totp::Algorithm::Sha1, label: None, issuer: None };
        t.code_at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())
    };
    // Signing in now needs the code too, so the key is made straight in SQL
    // terms through the same endpoint with the code.
    let (s, v) = call(&w, "/v1/auth/login", None, Some(json!({ "email": "a@example.com", "password": PASSWORD, "machine_name": "t" }))).await;
    let token = if v["token"].is_string() {
        v["token"].as_str().unwrap().to_string()
    } else {
        assert_eq!(s, StatusCode::OK);
        let (_, v) = call(&w, "/v1/auth/login/totp", None, Some(json!({ "challenge": v["challenge"], "code": code() }))).await;
        v["token"].as_str().unwrap().to_string()
    };
    let (mut m, auth) = material();
    m["password"] = json!(PASSWORD);
    let (s, v) = call(&w, "/v1/me/recovery", Some(&token), Some(m.clone())).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "totp_required"), "the code too, when the account has one");
    m["totp_code"] = json!(code());
    let (s, _) = call(&w, "/v1/me/recovery", Some(&token), Some(m)).await;
    assert_eq!(s, StatusCode::OK);

    let (_, v) = call(&w, "/v1/auth/recover/start", None, Some(json!({ "email": "a@example.com", "auth": auth }))).await;
    assert_eq!(v["factor"], "totp");
    let handle = json!({ "request": v["request"], "secret": v["secret"] });
    let mut bad = handle.clone();
    bad["code"] = json!("000000");
    for _ in 0..5 {
        call(&w, "/v1/auth/recover/verify", None, Some(bad.clone())).await;
    }
    let mut good = handle.clone();
    good["code"] = json!(code());
    let (s, v) = call(&w, "/v1/auth/recover/verify", None, Some(good)).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "recovery_closed"), "five wrong codes end the attempt");

    let (_, v) = call(&w, "/v1/auth/recover/start", None, Some(json!({ "email": "a@example.com", "auth": material().1 }))).await;
    assert!(v["request"].is_null(), "and a different key starts nothing");
}

#[tokio::test]
async fn the_owner_hands_over_only_to_an_admin_who_was_let_in() {
    let _g = DB_LOCK.lock().await;
    let Some(w) = world().await else { return eprintln!("SKIPPED: DATABASE_URL unset") };
    let owner = login(&w, "a@example.com", PASSWORD).await.unwrap();
    let admin = login(&w, "admin@example.com", PASSWORD).await.unwrap();
    let to = |user: &str, password: &str| json!({ "user_id": user, "password": password });

    let (s, v) = call(&w, "/v1/org/owner", Some(&admin), Some(to(ADMIN, PASSWORD))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "owner_only"));
    let (s, v) = call(&w, "/v1/org/owner", Some(&owner), Some(to(MEMBER, PASSWORD))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "not_an_admin"));
    let (s, v) = call(&w, "/v1/org/owner", Some(&owner), Some(to(ADMIN, "wrong"))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "wrong_password"));
    // An admin not yet given the organisation key could open nothing they own.
    sqlx::query("UPDATE org_members SET wrapped_ork = NULL WHERE user_id = $1::uuid").bind(ADMIN).execute(&w.pool).await.unwrap();
    let (s, v) = call(&w, "/v1/org/owner", Some(&owner), Some(to(ADMIN, PASSWORD))).await;
    assert_eq!((s, reason(&v)), (StatusCode::FORBIDDEN, "not_let_in"));
    sqlx::query("UPDATE org_members SET wrapped_ork = '\\x01' WHERE user_id = $1::uuid").bind(ADMIN).execute(&w.pool).await.unwrap();

    let (s, _) = call(&w, "/v1/org/owner", Some(&owner), Some(to(ADMIN, PASSWORD))).await;
    assert_eq!(s, StatusCode::OK);
    let roles: Vec<(String, String)> = sqlx::query_as("SELECT user_id::text, role::text FROM org_members WHERE org_id = $1::uuid ORDER BY user_id")
        .bind(ORG_A).fetch_all(&w.pool).await.unwrap();
    assert!(roles.contains(&(USER_A.into(), "admin".into())), "{roles:?}");
    assert!(roles.contains(&(ADMIN.into(), "owner".into())), "{roles:?}");
    assert_eq!(roles.iter().filter(|(_, r)| r == "owner").count(), 1, "still one owner");
}
