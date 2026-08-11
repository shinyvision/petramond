use super::*;

/// The record codec: positional identities round-trip, trailing empties
/// trim, and bytes this build cannot re-encode faithfully (the pre-socket
/// format, foreign bytes) refuse rather than guess.
#[test]
fn the_record_codec_round_trips_and_refuses_foreign_bytes() {
    let rec = Record {
        carved: 2,
        entries: vec![
            Entry::empty(),
            Entry::empty(),
            Entry {
                id: "forge:gold_inlay".into(),
                cond: 100,
                lvl: 0,
            },
            Entry::empty(),
        ],
    };
    let encoded = rec.encode();
    // Pristine Basic entries stay UNDECORATED — a pre-wear record and a
    // fresh stamp are byte-identical.
    assert_eq!(encoded, "2|,,forge:gold_inlay");
    let back = Record::parse(encoded.as_bytes()).unwrap();
    assert_eq!(back.carved, 2);
    assert_eq!(back.id_at(2), Some("forge:gold_inlay"));
    assert_eq!(back.entry_at(2).unwrap().cond, 100);
    assert_eq!(back.id_at(0), None);
    // The pre-socket format (a bare identity list) has no '|': refused.
    assert!(Record::parse(b"forge:diamond_tip").is_none());
    assert!(Record::parse(&[0xff, b'|']).is_none());
    // An absent key is a fresh tool, not a refusal.
    assert_eq!(Record::of_stack(&[]), Some(Record::default()));
}

/// The wear decorations: `@cond` and `^lvl` round-trip, omissions mean
/// pristine/Basic, and decorations this build cannot re-encode
/// faithfully (over-max condition, over-cap level, a bare `@`) refuse.
#[test]
fn the_record_codec_round_trips_condition_and_level() {
    let back = Record::parse(b"1|forge:diamond_tip@37^2,monsters:fang@0").unwrap();
    let tip = back.entry_at(0).unwrap();
    assert_eq!((tip.cond, tip.lvl), (37, 2));
    let fang = back.entry_at(1).unwrap();
    assert_eq!((fang.cond, fang.lvl), (0, 0));
    assert_eq!(back.encode(), "1|forge:diamond_tip@37^2,monsters:fang@0");

    // Omitted condition = full FOR THE LEVEL; full encodes undecorated.
    let lvl3 = Record::parse(b"0|forge:diamond_tip^3").unwrap();
    assert_eq!(lvl3.entry_at(0).unwrap().cond, 250);
    assert_eq!(lvl3.encode(), "0|forge:diamond_tip^3");

    // An upgraded-but-empty socket keeps its mount level.
    let empty_lvl = Record::parse(b"1|^2,forge:gold_inlay").unwrap();
    assert_eq!(empty_lvl.entry_at(0), None);
    assert_eq!(empty_lvl.entries[0].lvl, 2);
    assert_eq!(empty_lvl.encode(), "1|^2,forge:gold_inlay");

    // Refusals: condition past the level's max, a level past the cap,
    // and a condition on an empty socket.
    assert!(Record::parse(b"0|forge:diamond_tip@101").is_none());
    assert!(Record::parse(b"0|forge:diamond_tip^4").is_none());
    assert!(Record::parse(b"0|@50").is_none());
}

/// The six condition words band the CURRENT (level-scaled) maximum, and
/// the level words carry their tooltip palettes.
#[test]
fn condition_and_level_words_band_as_specified() {
    assert_eq!(condition_word(100, 0), ("Pristine", "green"));
    assert_eq!(condition_word(81, 0), ("Pristine", "green"));
    assert_eq!(condition_word(80, 0), ("Excellent", "green"));
    assert_eq!(condition_word(41, 0), ("Good", "yellow"));
    assert_eq!(condition_word(40, 0), ("Worn", "yellow"));
    assert_eq!(condition_word(1, 0), ("Damaged", "red"));
    assert_eq!(condition_word(0, 0), ("Broken", "red"));
    // Bands follow the level's max: 150 quanta is Pristine only at the
    // level that makes it full.
    assert_eq!(condition_word(150, 1), ("Pristine", "green"));
    assert_eq!(condition_word(150, 3), ("Good", "yellow"));
    assert_eq!(level_word(0).0, "Basic");
    assert_eq!(level_word(3), ("Legendary", "gold"));
}
