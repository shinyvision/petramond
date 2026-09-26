//! Worldgen byte-parity gate: hashes the production section pipeline over a
//! fixed sample (see `petramond_worldgen::parity`) and fails unless the hash
//! equals the checked-in `EXPECTED_COMBINED`.
//!
//! Run: `make genparity` (release-speed codegen, no packs installed).

use std::process::ExitCode;

use petramond_worldgen::parity;

fn main() -> ExitCode {
    let combined = parity::combined_hash();
    println!("COMBINED={combined:016x}");
    if combined == parity::EXPECTED_COMBINED {
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "worldgen output changed: expected COMBINED={:016x}.\n\
         A refactor, toolchain or rustflag change must reproduce the expected hash.\n\
         A change meant to alter generation sets `EXPECTED_COMBINED` in\n\
         crates/petramond-worldgen/src/parity.rs to the value above in the same commit.",
        parity::EXPECTED_COMBINED
    );
    ExitCode::FAILURE
}
