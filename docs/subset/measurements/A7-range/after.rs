fn mapped_address(response: &[u8]) -> Option<SocketAddr> {
    let kind = u16::from_be_bytes(response.get(0..2)?.try_into().ok()?);
    let magic = u32::from_be_bytes(response.get(4..8)?.try_into().ok()?);
    match (kind, magic) {
        (0x0101, STUN_MAGIC) => attribute(response.get(20..)?, None),
        _ => None,
    }
}

fn attribute(attrs: &[u8], plain: Option<SocketAddr>) -> Option<SocketAddr> {
    match attrs.get(0..4) {
        None => plain,
        Some(header) => {
            let kind = u16::from_be_bytes(header.get(0..2)?.try_into().ok()?);
            let len = usize::from(u16::from_be_bytes(header.get(2..4)?.try_into().ok()?));
            let value = attrs.get(4..len.checked_add(4)?)?;
            let rest = attrs.get(len.next_multiple_of(4).checked_add(4)?..).unwrap_or_default();
            match (kind, address(value)) {
                (0x0020, Some(mapped)) => Some(unxor(mapped)),
                (0x0001, Some(mapped)) => attribute(rest, Some(mapped)),
                _ => attribute(rest, plain),
            }
        }
    }
}

fn address(value: &[u8]) -> Option<SocketAddr> {
    let family = value.get(0..2)?;
    let port = u16::from_be_bytes(value.get(2..4)?.try_into().ok()?);
    let ip = Ipv4Addr::from(u32::from_be_bytes(value.get(4..8)?.try_into().ok()?));
    match (value.len(), family.get(1).copied()) {
        (8, Some(0x01)) => Some(SocketAddr::from((ip, port))),
        _ => None,
    }
}
