//! n0's relay map, named as browsers accept it.

/// n0's public relays, named without the root dot ([`crate::without_root_dot`]).
///
/// Every endpoint, host included, uses this map: a host advertises its home
/// relay to peers, and a browser that dials the dotted name fails `WebKit`'s
/// certificate check.
pub fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<iroh::RelayUrl>>()
        .iter()
        .map(crate::without_root_dot)
        .collect()
}
