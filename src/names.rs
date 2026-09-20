use rand::prelude::*;
use std::collections::HashSet;

const ADJECTIVES: &[&str] = &[
    "Happy",
    "Brave",
    "Clever",
    "Swift",
    "Gentle",
    "Mighty",
    "Wise",
    "Bold",
    "Bright",
    "Calm",
    "Eager",
    "Fair",
    "Jolly",
    "Kind",
    "Lively",
    "Noble",
    "Proud",
    "Quick",
    "Sharp",
    "Sunny",
    "Warm",
    "Young",
    "Zesty",
    "Cool",
    "Daring",
    "Elegant",
    "Fearless",
    "Graceful",
    "Humble",
    "Inventive",
];

const ANIMALS: &[&str] = &[
    "Panda", "Tiger", "Eagle", "Dolphin", "Wolf", "Fox", "Bear", "Lion", "Hawk", "Owl", "Raven",
    "Swan", "Deer", "Horse", "Otter", "Seal", "Whale", "Shark", "Turtle", "Rabbit", "Koala",
    "Penguin", "Giraffe", "Zebra", "Elephant", "Rhino", "Hippo", "Cheetah", "Jaguar", "Lynx",
];

const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

pub fn generate_room_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| *ID_ALPHABET.choose(&mut rng).expect("alphabet is non-empty") as char)
        .collect()
}

pub fn generate_unique_name(used_names: &HashSet<String>) -> String {
    let mut rng = rand::rng();

    for _ in 0..64 {
        let name = random_name(&mut rng);
        if !used_names.contains(&name) {
            return name;
        }
    }

    let base = random_name(&mut rng);
    (2..=used_names.len() + 2)
        .map(|n| format!("{base} {n}"))
        .find(|name| !used_names.contains(name))
        .unwrap_or(base)
}

fn random_name(rng: &mut impl Rng) -> String {
    let adj = ADJECTIVES.choose(rng).expect("non-empty");
    let animal = ANIMALS.choose(rng).expect("non-empty");
    format!("{adj} {animal}")
}
