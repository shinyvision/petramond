mod pins;
mod samples;

use crate::*;
use pins::PINS;
use samples::samples;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Samples(Vec<(&'static str, String)>);

impl Samples {
    fn pin<T: serde::Serialize>(&mut self, name: &'static str, value: &T) {
        self.0
            .push((name, hex(&encode(value).expect("wire pin sample encodes"))));
    }
}

#[test]
fn wire_format_matches_the_recorded_pins() {
    let actual = samples().0;
    let matches = actual.len() == PINS.len()
        && actual
            .iter()
            .zip(PINS)
            .all(|((name, bytes), (pin_name, pin_bytes))| name == pin_name && bytes == pin_bytes);
    if matches {
        return;
    }
    let mut block = String::new();
    for (name, bytes) in &actual {
        block.push_str(&format!("    (\"{name}\", \"{bytes}\"),\n"));
    }
    for ((name, bytes), (pin_name, pin_bytes)) in actual.iter().zip(PINS) {
        if name != pin_name {
            eprintln!("first divergence: sample '{name}' where the pin has '{pin_name}'");
            break;
        }
        if bytes != pin_bytes {
            eprintln!("first divergence: '{name}' encodes {bytes}, pinned {pin_bytes}");
            break;
        }
    }
    panic!(
        "the ABI wire format no longer matches its recorded pins \
         ({} samples, {} pins).\n\
         If this change is DELIBERATE (pre-release reshapes are allowed), replace the \
         body of PINS in mod-api/src/wire_pin/pins.rs with:\n\nconst PINS: &[(&str, &str)] = &[\n{block}];\n\n\
         and rebuild the mods (`make mods`). If it is NOT deliberate, a refactor just \
         reordered or reshaped ABI types — fix the refactor instead.",
        samples().0.len(),
        PINS.len(),
    );
}
