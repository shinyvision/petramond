use mod_api::RuntimeSide;

pub(in crate::modding) const HOST_CALL_BASE_FUEL: u64 = 1_000;

pub(in crate::modding) const HOST_CALL_FUEL_PER_BYTE: u64 = 4;

pub(in crate::modding) fn host_call_fuel(
    side: RuntimeSide,
    request_len: usize,
    reply_len: usize,
) -> u64 {
    if side == RuntimeSide::Client {
        return HOST_CALL_BASE_FUEL;
    }
    let bytes = (request_len as u64).saturating_add(reply_len as u64);
    HOST_CALL_BASE_FUEL.saturating_add(bytes.saturating_mul(HOST_CALL_FUEL_PER_BYTE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_calls_cost_a_base_plus_their_bytes() {
        let server = RuntimeSide::Server;
        assert_eq!(host_call_fuel(server, 0, 0), HOST_CALL_BASE_FUEL);
        assert_eq!(
            host_call_fuel(server, 10, 6),
            HOST_CALL_BASE_FUEL + 16 * HOST_CALL_FUEL_PER_BYTE
        );
        assert_eq!(host_call_fuel(server, usize::MAX, usize::MAX), u64::MAX);
    }

    #[test]
    fn a_client_read_costs_the_base_whatever_its_size() {
        let reply = 1 << 30;
        assert_eq!(
            host_call_fuel(RuntimeSide::Client, 64, reply),
            HOST_CALL_BASE_FUEL
        );
        assert!(host_call_fuel(RuntimeSide::Worldgen, 64, reply) > reply as u64);
    }
}
