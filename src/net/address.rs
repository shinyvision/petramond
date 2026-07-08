//! Server address parsing for the Connect to Server screen: a host with an
//! optional `:port`, defaulting to [`super::DEFAULT_PORT`]. Pure and
//! deterministic — DNS resolution happens later, on the connect worker thread.

use super::DEFAULT_PORT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddressError {
    Empty,
    InvalidPort,
    EmptyHost,
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressError::Empty => write!(f, "Enter a server address"),
            AddressError::InvalidPort => write!(f, "Invalid port"),
            AddressError::EmptyHost => write!(f, "Invalid address"),
        }
    }
}

/// Split `input` into `(host, port)`.
///
/// Rules: trimmed; empty → error. `[v6]:port` and `[v6]` bracket forms are
/// accepted (brackets stripped from the returned host). Two or more colons
/// WITHOUT brackets read as a bare IPv6 host with the default port — bracket
/// syntax is required to give an IPv6 address a port. Exactly one colon splits
/// host:port; the port must parse to a nonzero u16. No colon → default port.
pub(crate) fn parse_server_address(input: &str) -> Result<(String, u16), AddressError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AddressError::Empty);
    }

    // [v6] or [v6]:port
    if let Some(rest) = input.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err(AddressError::EmptyHost);
        };
        if host.is_empty() {
            return Err(AddressError::EmptyHost);
        }
        return match after {
            "" => Ok((host.to_string(), DEFAULT_PORT)),
            _ => match after.strip_prefix(':') {
                Some(port) => Ok((host.to_string(), parse_port(port)?)),
                None => Err(AddressError::EmptyHost),
            },
        };
    }

    match input.matches(':').count() {
        0 => Ok((input.to_string(), DEFAULT_PORT)),
        1 => {
            let (host, port) = input.split_once(':').expect("one colon");
            if host.is_empty() {
                return Err(AddressError::EmptyHost);
            }
            Ok((host.to_string(), parse_port(port)?))
        }
        // Bare IPv6 (e.g. `::1`, `fe80::…`): the whole string is the host.
        _ => Ok((input.to_string(), DEFAULT_PORT)),
    }
}

fn parse_port(s: &str) -> Result<u16, AddressError> {
    match s.parse::<u16>() {
        Ok(0) | Err(_) => Err(AddressError::InvalidPort),
        Ok(p) => Ok(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_colon_falls_back_to_the_default_port() {
        assert_eq!(
            parse_server_address("play.example.com"),
            Ok(("play.example.com".to_string(), DEFAULT_PORT))
        );
        assert_eq!(
            parse_server_address("  192.168.1.7  "),
            Ok(("192.168.1.7".to_string(), DEFAULT_PORT))
        );
    }

    #[test]
    fn one_colon_splits_host_and_port() {
        assert_eq!(
            parse_server_address("192.168.1.7:25"),
            Ok(("192.168.1.7".to_string(), 25))
        );
    }

    #[test]
    fn invalid_ports_are_rejected() {
        assert_eq!(
            parse_server_address("host:0"),
            Err(AddressError::InvalidPort)
        );
        assert_eq!(
            parse_server_address("host:70000"),
            Err(AddressError::InvalidPort)
        );
        assert_eq!(
            parse_server_address("host:junk"),
            Err(AddressError::InvalidPort)
        );
        assert_eq!(parse_server_address(":25"), Err(AddressError::EmptyHost));
    }

    #[test]
    fn ipv6_needs_brackets_to_carry_a_port() {
        assert_eq!(
            parse_server_address("[::1]:7434"),
            Ok(("::1".to_string(), 7434))
        );
        assert_eq!(
            parse_server_address("[fe80::2]"),
            Ok(("fe80::2".to_string(), DEFAULT_PORT))
        );
        // Bare multi-colon input is a whole IPv6 host on the default port.
        assert_eq!(
            parse_server_address("fe80::2:1"),
            Ok(("fe80::2:1".to_string(), DEFAULT_PORT))
        );
        assert_eq!(parse_server_address("[]"), Err(AddressError::EmptyHost));
        assert_eq!(parse_server_address("[::1"), Err(AddressError::EmptyHost));
    }

    #[test]
    fn empty_and_whitespace_are_rejected() {
        assert_eq!(parse_server_address(""), Err(AddressError::Empty));
        assert_eq!(parse_server_address("   "), Err(AddressError::Empty));
    }
}
