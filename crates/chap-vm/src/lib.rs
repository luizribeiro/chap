#![no_std]

//! Authority contract shared by VM hosts and plugins.

extern crate alloc;

#[cfg(feature = "host")]
extern crate std;

#[cfg(feature = "host")]
pub mod host;

#[lockgate_policy::capability("vm")]
pub mod vm {
    use alloc::{
        format,
        string::{String, ToString},
    };
    use core::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        str::FromStr,
    };
    use lockgate_policy::{Permission, Scope, ScopeError, ScopeRepr, ScopedPermission};

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum InstanceScope {
        Any,
        CreatedByCaller,
    }

    impl FromStr for InstanceScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "*" => Ok(Self::Any),
                "created-by-caller" => Ok(Self::CreatedByCaller),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for InstanceScope {
        fn canonical(&self) -> String {
            match self {
                Self::Any => "*".to_string(),
                Self::CreatedByCaller => "created-by-caller".to_string(),
            }
        }
    }

    impl Scope for InstanceScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner || matches!(self, Self::Any)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Mount {
        path: String,
        readonly_required: bool,
    }

    pub fn normalize_absolute_path(value: &str) -> Result<String, ScopeError> {
        if !value.starts_with('/') || value.contains('\0') {
            return Err(ScopeError::unknown(value));
        }

        let mut normalized = String::from("/");
        for component in value.split('/').filter(|component| !component.is_empty()) {
            if matches!(component, "." | "..") {
                return Err(ScopeError::unknown(value));
            }
            if normalized.len() > 1 {
                normalized.push('/');
            }
            normalized.push_str(component);
        }
        Ok(normalized)
    }

    impl Mount {
        pub fn path(&self) -> &str {
            &self.path
        }

        pub fn readonly(&self) -> bool {
            self.readonly_required
        }
    }

    impl FromStr for Mount {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            let (path, readonly_required) = match value.strip_prefix("ro:") {
                Some(path) => (path, true),
                None => (value, false),
            };
            let path = normalize_absolute_path(path).map_err(|_| ScopeError::unknown(value))?;

            Ok(Self {
                path,
                readonly_required,
            })
        }
    }

    impl ScopeRepr for Mount {
        fn canonical(&self) -> String {
            if self.readonly_required {
                format!("ro:{}", self.path)
            } else {
                self.path.clone()
            }
        }
    }

    impl Scope for Mount {
        fn contains(&self, inner: &Self) -> bool {
            path_covers(&self.path, &inner.path)
                && (!self.readonly_required || inner.readonly_required)
        }

        fn intersect(&self, other: &Self) -> Option<Self> {
            let path = if path_covers(&self.path, &other.path) {
                other.path.clone()
            } else if path_covers(&other.path, &self.path) {
                self.path.clone()
            } else {
                return None;
            };
            Some(Self {
                path,
                readonly_required: self.readonly_required || other.readonly_required,
            })
        }
    }

    fn path_covers(granted: &str, requested: &str) -> bool {
        granted == "/"
            || granted == requested
            || requested
                .strip_prefix(granted)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Egress {
        addr: IpAddr,
        prefix_len: u8,
        port: Option<u16>,
    }

    impl FromStr for Egress {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            let (addr_text, prefix_text, port_text, bracketed) = parse_egress_parts(value)?;
            let addr = IpAddr::from_str(addr_text).map_err(|_| ScopeError::unknown(value))?;
            if bracketed != matches!(addr, IpAddr::V6(_)) {
                return Err(ScopeError::unknown(value));
            }
            if let IpAddr::V6(addr) = addr {
                let ipv4_compatible =
                    addr.to_ipv4().is_some() && !addr.is_unspecified() && !addr.is_loopback();
                if addr.to_ipv4_mapped().is_some() || ipv4_compatible {
                    return Err(ScopeError::unknown(value));
                }
            }

            let host_prefix = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            let prefix_len = match prefix_text {
                Some(prefix)
                    if !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit()) =>
                {
                    prefix
                        .parse::<u8>()
                        .map_err(|_| ScopeError::unknown(value))?
                }
                Some(_) => return Err(ScopeError::unknown(value)),
                None => host_prefix,
            };
            if prefix_len > host_prefix {
                return Err(ScopeError::unknown(value));
            }
            if network_address(addr, prefix_len) != addr {
                return Err(ScopeError::unknown(value));
            }

            let port = match port_text {
                "*" => None,
                port if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
                    let port = port
                        .parse::<u16>()
                        .map_err(|_| ScopeError::unknown(value))?;
                    if port == 0 {
                        return Err(ScopeError::unknown(value));
                    }
                    Some(port)
                }
                _ => return Err(ScopeError::unknown(value)),
            };

            Ok(Self {
                addr,
                prefix_len,
                port,
            })
        }
    }

    impl ScopeRepr for Egress {
        fn canonical(&self) -> String {
            let port = self
                .port
                .map_or_else(|| "*".to_string(), |port| port.to_string());
            match self.addr {
                IpAddr::V4(addr) if self.prefix_len == 32 => format!("{addr}:{port}"),
                IpAddr::V4(addr) => format!("{addr}/{}:{port}", self.prefix_len),
                IpAddr::V6(addr) if self.prefix_len == 128 => format!("[{addr}]:{port}"),
                IpAddr::V6(addr) => format!("[{addr}]/{}:{port}", self.prefix_len),
            }
        }
    }

    #[cfg(feature = "microsandbox")]
    impl Egress {
        pub(crate) const fn addr(&self) -> IpAddr {
            self.addr
        }

        pub(crate) const fn prefix_len(&self) -> u8 {
            self.prefix_len
        }

        pub(crate) const fn port(&self) -> Option<u16> {
            self.port
        }
    }

    impl Scope for Egress {
        fn contains(&self, inner: &Self) -> bool {
            self.prefix_len <= inner.prefix_len
                && network_contains(self.addr, inner.addr, self.prefix_len)
                && (self.port.is_none() || self.port == inner.port)
        }

        fn intersect(&self, other: &Self) -> Option<Self> {
            let (addr, prefix_len) = if network_contains(self.addr, other.addr, self.prefix_len)
                && self.prefix_len <= other.prefix_len
            {
                (other.addr, other.prefix_len)
            } else if network_contains(other.addr, self.addr, other.prefix_len)
                && other.prefix_len <= self.prefix_len
            {
                (self.addr, self.prefix_len)
            } else {
                return None;
            };

            let port = match (self.port, other.port) {
                (None, port) | (port, None) => port,
                (Some(left), Some(right)) if left == right => Some(left),
                (Some(_), Some(_)) => return None,
            };
            Some(Self {
                addr,
                prefix_len,
                port,
            })
        }
    }

    fn parse_egress_parts(value: &str) -> Result<(&str, Option<&str>, &str, bool), ScopeError> {
        if value.starts_with('[') {
            let close = value.find(']').ok_or_else(|| ScopeError::unknown(value))?;
            let addr = &value[1..close];
            let suffix = &value[close + 1..];
            if let Some(port) = suffix.strip_prefix(':') {
                return Ok((addr, None, port, true));
            }
            let rest = suffix
                .strip_prefix('/')
                .ok_or_else(|| ScopeError::unknown(value))?;
            let (prefix, port) = rest
                .split_once(':')
                .ok_or_else(|| ScopeError::unknown(value))?;
            Ok((addr, Some(prefix), port, true))
        } else {
            let (addr_and_prefix, port) = value
                .rsplit_once(':')
                .ok_or_else(|| ScopeError::unknown(value))?;
            if addr_and_prefix.contains([':', '[', ']']) {
                return Err(ScopeError::unknown(value));
            }
            let (addr, prefix) = match addr_and_prefix.split_once('/') {
                Some((addr, prefix)) if !prefix.contains('/') => (addr, Some(prefix)),
                Some(_) => return Err(ScopeError::unknown(value)),
                None => (addr_and_prefix, None),
            };
            Ok((addr, prefix, port, false))
        }
    }

    fn network_address(addr: IpAddr, prefix_len: u8) -> IpAddr {
        match addr {
            IpAddr::V4(addr) => {
                IpAddr::V4(Ipv4Addr::from(u32::from(addr) & prefix_mask_v4(prefix_len)))
            }
            IpAddr::V6(addr) => IpAddr::V6(Ipv6Addr::from(
                u128::from(addr) & prefix_mask_v6(prefix_len),
            )),
        }
    }

    fn network_contains(granted: IpAddr, requested: IpAddr, prefix_len: u8) -> bool {
        match (granted, requested) {
            (IpAddr::V4(granted), IpAddr::V4(requested)) => {
                let mask = prefix_mask_v4(prefix_len);
                u32::from(granted) == u32::from(requested) & mask
            }
            (IpAddr::V6(granted), IpAddr::V6(requested)) => {
                let mask = prefix_mask_v6(prefix_len);
                u128::from(granted) == u128::from(requested) & mask
            }
            _ => false,
        }
    }

    fn prefix_mask_v4(prefix_len: u8) -> u32 {
        if prefix_len == 0 {
            0
        } else {
            u32::MAX << (32 - prefix_len)
        }
    }

    fn prefix_mask_v6(prefix_len: u8) -> u128 {
        if prefix_len == 0 {
            0
        } else {
            u128::MAX << (128 - prefix_len)
        }
    }

    /// Create/get-or-create a microVM (base "runs microVMs" disclosure line).
    pub const CREATE: Permission = Permission::new("create");

    /// Expose a host path to a VM (checked per-element of the mount list).
    pub const MOUNT: ScopedPermission<Mount> = ScopedPermission::new("mount");

    /// Reach a network destination from a VM (checked per-element of the egress list).
    pub const EGRESS: ScopedPermission<Egress> = ScopedPermission::new("egress");

    /// Operate on a VM you own (get/exec/read-file/write-file/destroy).
    pub const MANAGE: ScopedPermission<InstanceScope> = ScopedPermission::new("manage");
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;
    use lockgate_policy::{Scope, ScopeRepr, check_scope_laws};

    use super::vm::{Egress, InstanceScope, Mount, normalize_absolute_path};

    fn instance(value: &str) -> InstanceScope {
        InstanceScope::from_str(value).unwrap()
    }

    fn mount(value: &str) -> Mount {
        Mount::from_str(value).unwrap()
    }

    fn egress(value: &str) -> Egress {
        Egress::from_str(value).unwrap()
    }

    #[test]
    fn instance_scope_samples_obey_the_scope_laws() {
        check_scope_laws([instance("*"), instance("created-by-caller")]).unwrap();
    }

    #[test]
    fn instance_scope_containment_matches_identity_and_any() {
        let any = instance("*");
        let caller = instance("created-by-caller");

        for inner in [&any, &caller] {
            assert!(any.contains(inner));
        }
        assert!(caller.contains(&caller));
        assert!(!caller.contains(&any));
    }

    #[test]
    fn instance_scope_canonical_forms_round_trip() {
        for value in ["*", "created-by-caller"] {
            let parsed = instance(value);
            assert_eq!(parsed.canonical(), value);
            assert_eq!(
                InstanceScope::from_str(&parsed.canonical()).unwrap(),
                parsed
            );
        }
        for invalid in ["", concat!("pool", ":"), concat!("pool", ":x"), "builders"] {
            assert!(
                InstanceScope::from_str(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn mount_samples_obey_the_scope_laws() {
        check_scope_laws([
            mount("/"),
            mount("ro:/"),
            mount("/project"),
            mount("ro:/project"),
            mount("/project/src"),
            mount("ro:/project/src"),
            mount("/data"),
        ])
        .unwrap();
    }

    #[test]
    fn mount_containment_respects_boundaries_and_modes() {
        let cases = [
            ("/project", "/project", true),
            ("/project", "/project/x", true),
            ("/project", "/projectx", false),
            ("/project", "/Project", false),
            ("/project", "/", false),
            ("/", "/project/src", true),
            ("ro:/project", "ro:/project/x", true),
            ("ro:/project", "/project/x", false),
            ("/project", "ro:/project/x", true),
            ("/project／src", "/project／src", true),
            ("/project／src", "/project/src", false),
            ("/project/src", "/project／src", false),
            ("/caf\u{e9}", "/caf\u{e9}", true),
            ("/cafe\u{301}", "/cafe\u{301}", true),
            ("/caf\u{e9}", "/cafe\u{301}", false),
        ];
        for (granted, requested, expected) in cases {
            assert_eq!(
                mount(granted).contains(&mount(requested)),
                expected,
                "{granted} contains {requested}"
            );
        }
    }

    #[test]
    fn mount_parsing_normalizes_and_is_idempotent() {
        let cases = [
            ("/", "/", "/", false),
            ("///", "/", "/", false),
            ("//project///src//", "/project/src", "/project/src", false),
            (
                "ro://project///src//",
                "ro:/project/src",
                "/project/src",
                true,
            ),
            (
                "/Volumes/Backup 2024-01-01T10:00",
                "/Volumes/Backup 2024-01-01T10:00",
                "/Volumes/Backup 2024-01-01T10:00",
                false,
            ),
            (
                "ro:/Volumes/Backup 2024-01-01T10:00",
                "ro:/Volumes/Backup 2024-01-01T10:00",
                "/Volumes/Backup 2024-01-01T10:00",
                true,
            ),
            ("/project／src", "/project／src", "/project／src", false),
            ("/cafe\u{301}", "/cafe\u{301}", "/cafe\u{301}", false),
        ];
        for (input, canonical, path, readonly) in cases {
            let parsed = mount(input);
            assert_eq!(parsed.canonical(), canonical, "canonicalizing {input:?}");
            assert_eq!(parsed.path(), path, "path for {input:?}");
            assert_eq!(parsed.readonly(), readonly, "mode for {input:?}");

            let reparsed = Mount::from_str(&parsed.canonical()).unwrap();
            assert_eq!(reparsed, parsed, "round-tripping {input:?}");
            assert_eq!(reparsed.canonical(), canonical, "idempotence for {input:?}");
        }

        assert_eq!(
            normalize_absolute_path("//project///src//").unwrap(),
            "/project/src"
        );

        for invalid in [
            "",
            "project/src",
            "/project/../secret",
            "/project/./src",
            "rw:/project",
            "RO:/project",
            concat!("ro:", "ro:/x"),
            "ro:",
            "ro:relative",
            "/project/\0",
            "/project\0",
        ] {
            assert!(Mount::from_str(invalid).is_err(), "accepted {invalid:?}");
        }

        for invalid in [
            "relative",
            "/project/../secret",
            "/project/./src",
            "/project\0",
        ] {
            assert!(
                normalize_absolute_path(invalid).is_err(),
                "normalized {invalid:?}"
            );
        }
    }

    #[test]
    fn egress_samples_obey_the_scope_laws() {
        check_scope_laws([
            egress("0.0.0.0/0:*"),
            egress("0.0.0.0/0:443"),
            egress("199.232.0.0/16:*"),
            egress("199.232.0.0/16:443"),
            egress("199.232.12.34:443"),
            egress("199.232.12.34:80"),
            egress("[::]/0:*"),
            egress("[::1]:*"),
            egress("[2001:db8::]/32:*"),
            egress("[2001:db8::]/32:443"),
            egress("[64:ff9b::a00:1]:443"),
            egress("[2002:a00:1::]/48:443"),
        ])
        .unwrap();
    }

    #[test]
    fn egress_containment_respects_networks_and_ports() {
        let cases = [
            ("199.232.0.0/16:443", "199.232.12.34:443", true),
            ("199.232.0.0/16:443", "199.232.12.34:80", false),
            ("199.232.0.0/16:443", "8.8.8.8:443", false),
            ("10.0.0.1:*", "10.0.0.1:1", true),
            ("10.0.0.1:*", "10.0.0.1:65535", true),
            ("0.0.0.0/0:*", "203.0.113.10:8080", true),
            ("0.0.0.0/0:*", "[::1]:8080", false),
            ("[::]/0:*", "[2001:db8::1]:8080", true),
            ("[::]/0:*", "203.0.113.10:8080", false),
            ("10.0.0.1:443", "10.0.0.1:*", false),
            ("[2001:db8::]/32:443", "[2001:db8:1::1]:443", true),
            ("[2001:db8::]/32:443", "[2001:db9::1]:443", false),
            ("10.0.0.1/32:443", "10.0.0.0/31:443", false),
            ("10.0.0.1/32:443", "0.0.0.0/0:443", false),
        ];
        for (granted, requested, expected) in cases {
            assert_eq!(
                egress(granted).contains(&egress(requested)),
                expected,
                "{granted} contains {requested}"
            );
        }
    }

    #[test]
    fn egress_parsing_canonicalizes_and_is_idempotent() {
        let cases = [
            ("0.0.0.0/0:*", "0.0.0.0/0:*"),
            ("199.232.0.0/16:0443", "199.232.0.0/16:443"),
            ("10.0.0.0/8:443", "10.0.0.0/8:443"),
            ("10.0.0.1:1", "10.0.0.1:1"),
            ("10.0.0.1:80", "10.0.0.1:80"),
            ("10.0.0.1:65535", "10.0.0.1:65535"),
            ("[::]/0:*", "[::]/0:*"),
            ("[::1]:*", "[::1]:*"),
            ("[2001:0db8::]/32:443", "[2001:db8::]/32:443"),
            ("[64:ff9b::a00:1]:443", "[64:ff9b::a00:1]:443"),
            ("[2002:a00:1::]/48:443", "[2002:a00:1::]/48:443"),
        ];
        for (input, canonical) in cases {
            let parsed = egress(input);
            assert_eq!(parsed.canonical(), canonical);
            let reparsed = Egress::from_str(&parsed.canonical()).unwrap();
            assert_eq!(reparsed, parsed);
            assert_eq!(reparsed.canonical(), canonical);
        }
    }

    #[test]
    fn egress_parsing_rejects_malformed_inputs() {
        for invalid in [
            "",
            "10.0.0.1",
            "10.0.0.1:",
            "10.0.0.1:0",
            "10.0.0.1:65536",
            "10.0.0.1:-1",
            "10.0.0.1/33:80",
            "10.0.0.1/8:443",
            "192.168.1.1/1:443",
            "010.0.0.1:443",
            "host.example:443",
            "::1:443",
            "2001:db8::1:443",
            "fe80::1%en0",
            "[::1",
            "[::1]:",
            "[::1]/129:443",
            "[fe80::1%en0]:443",
            "[10.0.0.1]:443",
            "[::ffff:10.0.0.1]:443",
            "[::10.0.0.1]:443",
            "[::ffff:0:0]/96:*",
            "[::ffff:169.254.169.254]:443",
        ] {
            assert!(Egress::from_str(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn ipv6_any_network_has_no_ipv4_mapped_witness() {
        let any_v6 = egress("[::]/0:443");
        assert!(any_v6.contains(&egress("[2001:db8::1]:443")));
        assert!(Egress::from_str("[::ffff:10.0.0.1]:443").is_err());
    }
}
