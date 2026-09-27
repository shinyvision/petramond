use mod_api::{AbiVersion, Capabilities, GuestCall, GuestRet, HostRet, ABI_VERSION};
use wasmtime::Module;

use super::super::instance::{wat_abi_exports, ModInstance};
use super::wat_bytes;

const UNIT_REPLY: &str = "(i64.const 2199023255553)";

fn module(handshake: &str, data: &str, init: &str, dispatch: &str) -> Module {
    assert_eq!(mod_api::pack_ptr_len(512, 1), 2199023255553);
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 512) "\00")
{data}{handshake}  (func (export "mod_init") (param i32 i64)
    {init})
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    {dispatch}))"#
    );
    Module::new(super::super::host::engine(), wat.as_bytes()).expect("assemble ABI guest")
}

fn instantiate(handshake: &str) -> Result<ModInstance, String> {
    ModInstance::from_module("abi", &module(handshake, "", "", UNIT_REPLY), 1)
}

fn refusal(handshake: &str) -> String {
    instantiate(handshake)
        .err()
        .expect("the handshake refuses this guest")
}

fn requiring(version: AbiVersion, requires: Capabilities) -> String {
    format!(
        "  (func (export \"mod_abi_version\") (result i32) (i32.const {}))\n  \
         (func (export \"mod_abi_requires\") (result i64) (i64.const {}))\n",
        version.pack() as i32,
        requires.bits() as i64
    )
}

#[test]
fn a_guest_of_the_host_abi_loads_and_initializes() {
    let mut inst = instantiate(&wat_abi_exports(ABI_VERSION)).expect("same ABI loads");
    inst.call_init_detached();
    assert!(!inst.disabled());
    assert_eq!(inst.dispatches(), 1);
}

#[test]
fn a_guest_of_another_minor_of_the_same_major_loads() {
    for minor in [ABI_VERSION.minor + 1, u16::MAX] {
        let version = AbiVersion {
            minor,
            ..ABI_VERSION
        };
        assert!(instantiate(&wat_abi_exports(version)).is_ok(), "{version}");
    }
}

#[test]
fn a_guest_built_for_an_older_major_is_refused_at_load() {
    let old = AbiVersion {
        major: ABI_VERSION.major - 1,
        minor: 7,
    };
    let why = refusal(&wat_abi_exports(old));
    assert!(why.starts_with("incompatible mod"), "{why}");
    assert!(why.contains(&format!("built for mod ABI {old}")), "{why}");
    assert!(why.contains("rebuild the mod"), "{why}");
}

#[test]
fn a_guest_built_for_a_newer_major_is_refused_at_load() {
    let new = AbiVersion {
        major: ABI_VERSION.major + 1,
        minor: 0,
    };
    let why = refusal(&wat_abi_exports(new));
    assert!(why.contains(&format!("built for mod ABI {new}")), "{why}");
    assert!(why.contains("update the game"), "{why}");
    let bare = format!(
        "  (func (export \"mod_abi_version\") (result i32) (i32.const {}))\n",
        new.pack() as i32
    );
    assert_eq!(refusal(&bare), why);
}

#[test]
fn a_guest_without_an_abi_version_is_refused_at_load() {
    let why = refusal("");
    assert!(why.contains("declares no mod ABI version"), "{why}");
}

#[test]
fn a_guest_requiring_capabilities_the_host_lacks_is_refused_at_load() {
    let future = Capabilities::from_bits(1 << 63);
    let why = refusal(&requiring(
        ABI_VERSION,
        future.union(Capabilities::WORLDGEN),
    ));
    assert!(why.contains("does not provide: bit 63"), "{why}");
    assert!(instantiate(&requiring(ABI_VERSION, mod_api::HOST_CAPABILITIES)).is_ok());
}

#[test]
fn mod_init_receives_the_host_version_and_capabilities() {
    let init = format!(
        "(if (i32.ne (local.get 0) (i32.const {})) (then unreachable))\n    \
         (if (i64.ne (local.get 1) (i64.const {})) (then unreachable))",
        ABI_VERSION.pack() as i32,
        mod_api::HOST_CAPABILITIES.bits() as i64
    );
    let module = module(&wat_abi_exports(ABI_VERSION), "", &init, UNIT_REPLY);
    let mut inst = ModInstance::from_module("abi", &module, 1).expect("loads");
    inst.call_init_detached();
    assert!(!inst.disabled(), "init saw the host's handshake values");
}

#[test]
fn a_guest_declining_a_call_as_unsupported_stays_enabled() {
    let unsupported = mod_api::encode(&GuestRet::Unsupported).unwrap();
    let data = format!(
        "  (data (i32.const 1024) \"{}\")\n",
        wat_bytes(&unsupported)
    );
    let reply = mod_api::pack_ptr_len(1024, unsupported.len() as u32);
    let module = module(
        &wat_abi_exports(ABI_VERSION),
        &data,
        "",
        &format!("(i64.const {reply})"),
    );
    let mut inst = ModInstance::from_module("abi", &module, 1).expect("loads");
    inst.call_init_detached();
    for _ in 0..2 {
        assert_eq!(
            inst.call_guest_detached(&GuestCall::TickSystem { id: 1 }),
            None
        );
        assert!(!inst.disabled(), "a decline is not a protocol break");
    }
}

/// A newer guest's host call past the end of `HostCall` gets
/// `HostRet::Unsupported` instead of trapping: this guest issues one and traps
/// unless the reply the host staged at its scratch (4096) is exactly that.
#[test]
fn an_unknown_host_call_is_answered_unsupported() {
    let unknown = [0xff, 0x7f];
    let expected = mod_api::encode(&HostRet::Unsupported).unwrap();
    assert_eq!(expected.len(), 1);
    let data = format!("  (data (i32.const 1024) \"{}\")\n", wat_bytes(&unknown));
    let dispatch = format!(
        "(drop (call $hd (i32.const 1024) (i32.const {})))\n    \
         (if (i32.ne (i32.load8_u (i32.const 4096)) (i32.const {})) (then unreachable))\n    \
         {UNIT_REPLY}",
        unknown.len(),
        expected[0]
    );
    let module = module(&wat_abi_exports(ABI_VERSION), &data, "", &dispatch);
    let mut inst = ModInstance::from_module("abi", &module, 1).expect("loads");
    inst.call_init_detached();
    assert_eq!(
        inst.call_guest_detached(&GuestCall::TickSystem { id: 1 }),
        Some(GuestRet::Unit)
    );
    assert!(!inst.disabled());
}
