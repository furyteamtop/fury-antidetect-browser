//! Prints an Argon2id hash for seeding a development database.
//! Never used in production: real users are created through the API.
fn main() {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let password = std::env::args().nth(1).expect("usage: hash <password>");
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    println!(
        "{}",
        argon2::Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
    );
}
