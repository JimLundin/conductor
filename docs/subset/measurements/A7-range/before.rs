fn mapped_address(response: &[u8]) -> Option<SocketAddr> {
    let (head, rest) = response.split_at_checked(2)?;
    let kind = u16::from_be_bytes(head.try_into().ok()?);
    let (_, rest) = rest.split_at_checked(2)?;
    let (head, rest) = rest.split_at_checked(4)?;
    let magic = u32::from_be_bytes(head.try_into().ok()?);
    let (_, attrs) = rest.split_at_checked(12)?;
    match (kind, magic) {
        (0x0101, STUN_MAGIC) => attribute(attrs, None),
        _ => None,
    }
}

fn attribute(attrs: &[u8], plain: Option<SocketAddr>) -> Option<SocketAddr> {
    match attrs.split_at_checked(4) {
        None => plain,
        Some((header, body)) => {
            let (head, tail) = header.split_at_checked(2)?;
            let kind = u16::from_be_bytes(head.try_into().ok()?);
            let len = usize::from(u16::from_be_bytes(tail.try_into().ok()?));
            let (value, _) = body.split_at_checked(len)?;
            let rest = body.split_at_checked(len.next_multiple_of(4)).map(|(_, rest)| rest).unwrap_or_default();
            match (kind, address(value)) {
                (0x0020, Some(mapped)) => Some(unxor(mapped)),
                (0x0001, Some(mapped)) => attribute(rest, Some(mapped)),
                _ => attribute(rest, plain),
            }
        }
    }
}

fn address(value: &[u8]) -> Option<SocketAddr> {
    let (family, rest) = value.split_at_checked(2)?;
    let (port, ip) = rest.split_at_checked(2)?;
    let port = u16::from_be_bytes(port.try_into().ok()?);
    let ip = Ipv4Addr::from(u32::from_be_bytes(ip.try_into().ok()?));
    match (value.len(), family.get(1).copied()) {
        (8, Some(0x01)) => Some(SocketAddr::from((ip, port))),
        _ => None,
    }
}
