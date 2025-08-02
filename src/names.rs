use rand::prelude::*;

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

pub fn generate_unique_name(used_names: &[String]) -> String {
    let mut rng = rand::rng();
    let mut attempts = 0;

    loop {
        let adj = ADJECTIVES.choose(&mut rng).unwrap();
        let animal = ANIMALS.choose(&mut rng).unwrap();
        let name = format!("{adj} {animal}");

        if !used_names.contains(&name) || attempts > 100 {
            return name;
        }

        attempts += 1;
    }
}
