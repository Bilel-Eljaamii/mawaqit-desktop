//! In-tree mutation fuzzer for the mawaqit-api surface that the pinned
//! corpus (`hostile_corpus.rs`) does not reach: the search-response model,
//! the URL builder, and the disk snapshot layer.
//!
//! Method: start from a known-valid seed (a real search response, a real
//! stored snapshot), then apply deterministic mutations — truncation, byte
//! flips, chunk splicing, duplication — driven by a fixed xorshift seed, so
//! every run explores the same space. The invariants are the hostile-data
//! contract: nothing panics, everything hostile degrades to `Err`/`None`
//! or a well-formed value, and whatever *does* parse must survive the
//! downstream pipeline.
//!
//! `cargo test -p mawaqit-api --test hostile_fuzz` (offline, fast, CI-safe).

use std::path::PathBuf;

use chrono::Datelike;
use mawaqit_api::{disk, page_url, parse_page, ConfData, Mosque};
use serde_json::json;

// ------------------------------------------------------------- mutation engine

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// Bytes that tend to break parsers: JSON structure, escapes, sign/number
/// forms, UTF-8 lead bytes, path metacharacters.
const INTERESTING: &[u8] = &[
    b'"', b'\\', b'{', b'}', b'[', b']', b':', b',', b'+', b'-', b'.', b'/', b'?', b'#',
    b'@', b'0', b'9', b' ', b'\n', b'\t', 0x00, 0x7F, 0x80, 0xC3, 0xE2, 0xAC, 0xFF,
];

/// One deterministic mutation of `data`.
fn mutate(rng: &mut Rng, data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    match rng.below(4) {
        // truncate at a random point
        0 => {
            let cut = rng.below(v.len() + 1);
            v.truncate(cut);
        }
        // flip 1..=8 bytes
        1 => {
            let flips = rng.below(8) + 1;
            for _ in 0..flips {
                if v.is_empty() {
                    break;
                }
                let i = rng.below(v.len());
                v[i] = (rng.next() % 256) as u8;
            }
        }
        // splice in a run of interesting bytes
        2 => {
            let at = rng.below(v.len() + 1);
            let n = rng.below(16) + 1;
            for k in 0..n {
                let b = INTERESTING[rng.below(INTERESTING.len())];
                v.insert((at + k).min(v.len()), b);
            }
        }
        // duplicate a random slice somewhere else
        _ => {
            if !v.is_empty() {
                let a = rng.below(v.len());
                let b = (a + rng.below(v.len() - a + 1)).min(v.len());
                let slice = v[a..b].to_vec();
                let at = rng.below(v.len() + 1);
                for (k, byte) in slice.iter().enumerate() {
                    v.insert((at + k).min(v.len()), *byte);
                }
            }
        }
    }
    v
}

/// Run `f` over `iters` mutations of every seed.
fn for_each_mutation(seeds: &[&[u8]], iters_per_seed: usize, mut f: impl FnMut(&[u8])) {
    let mut rng = Rng::new(0x9E_37_79_B9_7F_4A_7C_15);
    for seed in seeds {
        for _ in 0..iters_per_seed {
            f(&mutate(&mut rng, seed));
        }
    }
}

// ------------------------------------------------------------------- helpers

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mawaqit-fuzz-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A realistic search response: full entries, sparse entries, odd but
/// legal field shapes.
const SEARCH_SEED: &[u8] = r#"[{"uuid":"u1","id":12,"slug":"grande-mosquee-de-paris","name":"Grande Mosquée","label":"GMdP","locality":"Paris","country":"France","extra":{"verified":true}},{"slug":"a"},{"label":"l","locality":"Lyon"},{"slug":"ᴍᴏꜱQᴜᴇ","name":".."},{"id":{"deep":[1,2]}}]"#.as_bytes();

/// Store a rich ConfData through the real serializer and return the exact
/// bytes on disk — the snapshot seed.
fn snapshot_seed(dir: &std::path::Path, slug: &str) -> Vec<u8> {
    let raw = json!({
        "times": ["06:30", "08:00", "13:00", "15:30", "17:45", "19:15"],
        "shuruq": "08:00",
        "calendar": [{"1": ["06:30","08:00","13:00","15:30","17:45","19:15"],
                      "28": ["06:31","08:01","13:01","15:31","17:46","19:16"]}],
        "iqamaCalendar": [{"1": ["+10", "+15", "13:20", "+20", "18:00"]}],
        "name": "Seed Mosque",
        "jumua": "13:50",
        "image": "https://pics.test/seed.jpg",
        "announcements": [{"id": 1, "title": "Hello"}],
        "unmodeled": {"nested": [1, 2, 3]}
    });
    let page = format!("<script>var confData = {raw};</script>");
    let conf = parse_page(&page, "fuzz-seed").expect("seed page parses");
    disk::store(dir, slug, &conf).expect("seed stores");
    std::fs::read(disk::snapshot_path(dir, slug)).expect("seed file exists")
}

fn dig(conf: &ConfData) {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let _ = mawaqit_api::month_times(conf, 1);
    let _ = mawaqit_api::month_times(conf, 12);
    let _ = mawaqit_api::month_iqama_times(conf, 7);
    let _ = mawaqit_api::times_for_date(conf, date);
}

// --------------------------------------------------------------------- tests

#[test]
fn fuzz_search_response_parsing_never_panics() {
    for_each_mutation(&[SEARCH_SEED], 2500, |bytes| {
        if let Ok(mosques) = serde_json::from_slice::<Vec<Mosque>>(bytes) {
            for m in &mosques {
                // Whatever the shape, the display accessors are total.
                let _ = m.display_name().len();
                let _ = m.place();
                if let Some(slug) = m.mosque_id() {
                    // The URL builder is total for any slug; the secure
                    // confinement contract is asserted in hostile_http F2.
                    let _ = page_url("https://mawaqit.net", slug).len();
                    let _ = mawaqit_api::minutes_between(slug, "13:00");
                }
            }
        }
    });
}

#[test]
fn fuzz_disk_snapshot_load_never_panics() {
    let dir = temp_dir("load");
    let seed = snapshot_seed(&dir, "seed-mosque");
    for_each_mutation(&[&seed], 2500, |bytes| {
        let path = disk::snapshot_path(&dir, "seed-mosque");
        std::fs::write(&path, bytes).expect("write snapshot");
        if let Some((fetched_at, conf)) = disk::load(&dir, "seed-mosque") {
            // Whatever loads must be structurally alive: a real date and a
            // ConfData the calendar pipeline can digest without panicking.
            assert_eq!(fetched_at.year(), 2026);
            dig(&conf);
        }
    });
    // Sanity: the pristine seed still loads after the fuzz run clobbered
    // the file with mutations.
    std::fs::write(disk::snapshot_path(&dir, "seed-mosque"), &seed)
        .expect("rewrite seed");
    assert!(disk::load(&dir, "seed-mosque").is_some(), "seed must survive the fuzz run");
}

/// FINDING F10 (found by this fuzzer's seed) — the snapshot round-trip drops
/// wire-tolerated confData. The live path (`parse_page`) tolerates oddities
/// by design: `null` entries in calendar rows drop the iqama calendar,
/// non-string display fields collapse to `None` — but the *raw* object kept
/// in `ConfData.raw` still carries them, and `disk::load` deserializes that
/// raw object through strict serde. Result: a mosque whose live page parses
/// fine stores a snapshot that never loads — offline mode silently dead for
/// exactly the messy real-world mosques, plus a refetch every launch.
/// FIX: on `load`, fall back to a scraper-style tolerant extraction from
/// `envelope.conf` when strict serde fails (or sanitize `raw` at store
/// time), then un-ignore.
#[test]
fn finding_f10_snapshot_roundtrips_wire_tolerated_shapes() {
    let dir = temp_dir("f10");
    // The page parses (scraper drops the broken iqama calendar and the
    // numeric name), so the app stores a snapshot for it.
    let page = concat!(
        r#"<script>var confData = {"times":["06:30","08:00","13:00","15:30","17:45"],"#,
        r#""calendar":[{"1":["06:30","08:00","13:00","15:30","17:45","19:15"]}],"#,
        r#""iqamaCalendar":[{"1":["+10", null, "13:20", "+20", "18:00"]}], "name": 12345};"#,
        r#"</script>"#
    );
    let conf = parse_page(page, "f10").expect("live path parses this page");
    disk::store(&dir, "messy-mosque", &conf).expect("snapshot stored");

    // The offline path must serve what the online path accepted.
    let (_, loaded) = disk::load(&dir, "messy-mosque")
        .expect("snapshot of a parseable page must reload");
    assert_eq!(loaded.times.len(), 5);
}

#[test]
fn fuzz_disk_store_load_roundtrip_under_hostile_slugs() {
    let dir = temp_dir("roundtrip");
    let conf_seed = br#"{"times":["06:30","08:00","13:00","15:30","17:45"],"calendar":[{"1":["06:30","08:00","13:00","15:30","17:45","19:15"]}],"name":"X"}"#;

    let hostile_slugs: Vec<String> = vec![
        "grande-mosquee-de-paris".into(),
        String::new(),
        "../etc/passwd".into(),
        "a/b/c".into(),
        "\u{0000}\u{202E}\u{FEFF}".into(),
        "🕌".repeat(100),
        "x".repeat(100_000),
    ];

    let mut rng = Rng::new(0x5E_ED_5E_ED_5E_ED_5E_ED);
    for slug in &hostile_slugs {
        for _ in 0..400 {
            let bytes = mutate(&mut rng, conf_seed);
            let page = format!(
                "<script>var confData = {};</script>",
                String::from_utf8_lossy(&bytes)
            );
            let Ok(conf) = parse_page(&page, "fuzz") else { continue };

            // The hash-derived file name must never escape the directory,
            // whatever the slug.
            let path = disk::snapshot_path(&dir, slug);
            assert_eq!(path.parent(), Some(dir.as_path()), "slug {slug:?} escaped");
            assert_eq!(path.extension().and_then(|e| e.to_str()), Some("json"));

            if let Some(stored_at) = disk::store(&dir, slug, &conf) {
                assert_eq!(stored_at.year(), 2026);
                // Store-then-load must agree: same slug round-trips (or
                // degrades to None), never a foreign snapshot.
                if let Some((fetched_at, loaded)) = disk::load(&dir, slug) {
                    assert_eq!(
                        fetched_at, stored_at,
                        "slug {slug:?} fetched_at mismatch"
                    );
                    dig(&loaded);
                }
            }
        }
    }
}

#[test]
fn fuzz_search_and_snapshot_coexist_bounded() {
    // Resource-exhaustion shape: a response that is legal JSON but huge, and
    // a snapshot whose conf value is a multi-megabyte string. Parsing must
    // stay bounded by the response cap, and the snapshot layer must swallow
    // whatever it cannot represent.
    let big = json!([{ "slug": "s", "name": "N".repeat(1_000_000) }]).to_string();
    if let Ok(mosques) = serde_json::from_str::<Vec<Mosque>>(&big) {
        assert_eq!(mosques.len(), 1);
        assert_eq!(mosques[0].display_name().len(), 1_000_000);
    }

    let dir = temp_dir("big-string");
    let conf = ConfData {
        raw: json!({ "times": ["06:30", "07:00", "12:00", "15:00", "18:00", "20:00"], "blob": "Z".repeat(2_000_000) }),
        ..Default::default()
    };
    if disk::store(&dir, "big", &conf).is_some() {
        assert!(disk::load(&dir, "big").is_some(), "big-but-legal snapshot must reload");
    }
}
