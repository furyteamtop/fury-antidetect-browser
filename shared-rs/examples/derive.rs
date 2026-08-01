
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let p: fury_shared::Persona = serde_json::from_str(&text).unwrap();
    p.validate().expect("persona is inconsistent");
    let ctx = fury_shared::ProfileContext {
        timezone: "America/New_York".into(),
        languages: vec!["en-US".into(), "en".into()],
        ui_locale: "en-US".into(),
        chrome_major: 150,
        chrome_full_version: "150.0.7871.187".into(),
    };
    let seed: u64 = std::env::args().nth(2).unwrap().parse().unwrap();
    println!("{}", serde_json::to_string_pretty(&p.derive_core_config(seed, &ctx)).unwrap());
}
