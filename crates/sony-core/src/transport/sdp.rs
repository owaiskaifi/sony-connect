//! Minimal SDP client pieces (pure, platform-independent): build a
//! ServiceSearchAttributeRequest for one 128-bit UUID and pull the RFCOMM
//! channel out of the response's ProtocolDescriptorList.
//!
//! This replaces libbluetooth's `sdp_service_search_attr_req` +
//! `sdp_get_proto_port(.., RFCOMM_UUID)` used by the reference Linux code.

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Nil,
    Uint(u64),
    Int(i64),
    Uuid(Vec<u8>),
    Text(Vec<u8>),
    Bool(bool),
    Seq(Vec<Element>),
    Other,
}

/// Parse a UUID string ("956c7b26-d49a-...") into 16 bytes.
pub fn parse_uuid128(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// ServiceSearchAttributeRequest PDU (0x06) asking for all attributes.
pub fn search_attr_request(tid: u16, uuid: &[u8; 16], continuation: &[u8]) -> Vec<u8> {
    let mut params = vec![0x35, 0x11, 0x1C];
    params.extend_from_slice(uuid);
    params.extend_from_slice(&[0xFF, 0xFF]); // max attribute byte count
    params.extend_from_slice(&[0x35, 0x05, 0x0A, 0x00, 0x00, 0xFF, 0xFF]); // 0x0000-0xFFFF
    if continuation.is_empty() {
        params.push(0x00);
    } else {
        params.extend_from_slice(continuation);
    }
    let mut pdu = vec![0x06];
    pdu.extend_from_slice(&tid.to_be_bytes());
    pdu.extend_from_slice(&(params.len() as u16).to_be_bytes());
    pdu.extend(params);
    pdu
}

/// Parse a ServiceSearchAttributeResponse PDU (0x07).
/// Returns (attribute list bytes chunk, continuation state incl. its length byte;
/// empty when complete).
pub fn parse_search_attr_response(pdu: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if pdu.len() < 7 || pdu[0] != 0x07 {
        return None;
    }
    let count = u16::from_be_bytes([pdu[5], pdu[6]]) as usize;
    let data = pdu.get(7..7 + count)?.to_vec();
    let cont_len = *pdu.get(7 + count)? as usize;
    let cont = if cont_len == 0 {
        Vec::new()
    } else {
        pdu.get(7 + count..7 + count + 1 + cont_len)?.to_vec()
    };
    Some((data, cont))
}

/// Parse one data element; returns (element, bytes consumed).
pub fn parse_element(b: &[u8]) -> Option<(Element, usize)> {
    let h = *b.first()?;
    let ty = h >> 3;
    let sz = h & 7;
    let (len, hdr) = match (ty, sz) {
        (0, _) => (0usize, 1usize),
        (_, 0) => (1, 1),
        (_, 1) => (2, 1),
        (_, 2) => (4, 1),
        (_, 3) => (8, 1),
        (_, 4) => (16, 1),
        (_, 5) => (*b.get(1)? as usize, 2),
        (_, 6) => (u16::from_be_bytes([*b.get(1)?, *b.get(2)?]) as usize, 3),
        (_, 7) => (
            u32::from_be_bytes([*b.get(1)?, *b.get(2)?, *b.get(3)?, *b.get(4)?]) as usize,
            5,
        ),
        _ => return None,
    };
    let body = b.get(hdr..hdr + len)?;
    let be = |x: &[u8]| x.iter().take(8).fold(0u64, |a, v| (a << 8) | *v as u64);
    let el = match ty {
        0 => Element::Nil,
        1 => Element::Uint(be(body)),
        2 => Element::Int(be(body) as i64),
        3 => Element::Uuid(body.to_vec()),
        4 | 8 => Element::Text(body.to_vec()),
        5 => Element::Bool(body.first().copied().unwrap_or(0) != 0),
        6 | 7 => {
            let mut items = Vec::new();
            let mut off = 0;
            while off < body.len() {
                let (e, n) = parse_element(&body[off..])?;
                items.push(e);
                off += n;
            }
            Element::Seq(items)
        }
        _ => Element::Other,
    };
    Some((el, hdr + len))
}

fn is_rfcomm_uuid(u: &[u8]) -> bool {
    match u.len() {
        2 => u == [0x00, 0x03],
        4 => u == [0x00, 0x00, 0x00, 0x03],
        16 => u[..4] == [0x00, 0x00, 0x00, 0x03],
        _ => false,
    }
}

/// Find the RFCOMM channel in a complete attribute-lists blob.
pub fn rfcomm_channel(attr_lists: &[u8]) -> Option<u8> {
    let (root, _) = parse_element(attr_lists)?;
    let records = match root {
        Element::Seq(r) => r,
        _ => return None,
    };
    for rec in records {
        let Element::Seq(attrs) = rec else { continue };
        for pair in attrs.chunks(2) {
            // ProtocolDescriptorList = 0x0004
            if let [Element::Uint(0x0004), Element::Seq(protos)] = pair {
                for proto in protos {
                    if let Element::Seq(p) = proto {
                        if let [Element::Uuid(u), Element::Uint(ch), ..] = p.as_slice() {
                            if is_rfcomm_uuid(u) {
                                return Some(*ch as u8);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Attribute lists returned by a ULT WEAR for the MDR v2 UUID.
    const ULT_WEAR: &str = "36005c3600590900000a2000000009000135111c956c7b26d49a4ba8b03fb17d393cb6e2090004350c3503190100350519000308120900053503191002090006350909656e09006a090100090100250f5350502053657269616c20506f7274";

    #[test]
    fn ult_wear_channel() {
        assert_eq!(rfcomm_channel(&hex(ULT_WEAR)), Some(18));
    }

    #[test]
    fn request_layout() {
        let u = parse_uuid128(crate::protocol::SERVICE_UUID_V2).unwrap();
        let r = search_attr_request(1, &u, &[]);
        assert_eq!(&r[..5], &[0x06, 0x00, 0x01, 0x00, 0x1d]);
        assert_eq!(r.len(), 5 + 0x1d);
        assert_eq!(*r.last().unwrap(), 0x00);
    }

    #[test]
    fn response_parse() {
        let mut pdu = vec![
            0x07, 0x00, 0x01, 0x00, 0x06, 0x00, 0x03, 0x35, 0x01, 0x00, 0x00,
        ];
        let (d, c) = parse_search_attr_response(&pdu).unwrap();
        assert_eq!(d, vec![0x35, 0x01, 0x00]);
        assert!(c.is_empty());
        pdu.pop();
        pdu.extend([0x02, 0xaa, 0xbb]);
        let (_, c) = parse_search_attr_response(&pdu).unwrap();
        assert_eq!(c, vec![0x02, 0xaa, 0xbb]);
    }
}
