// Copyright Aliaksei Levin (levlam@telegram.org), Arseny Smirnov (arseny30@gmail.com) 2014-2026.
// Rust adaptation distributed under the Boost Software License 1.0; see LICENSE_1_0.txt.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use std::ops::Range;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Incomplete packet")]
    Incomplete,
    #[error("Invalid packet length: {0}")]
    Length(usize),
    #[error("Invalid TL value")]
    Invalid,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Packet { data: Range<usize>, consumed: usize },
    QuickAck(u32),
}
pub const INTERMEDIATE_TAG: [u8; 4] = [0xee; 4];
pub const MAX_PACKET: usize = (1 << 24) - 1;

/// Read one frame without consuming incomplete input. Mirrors IntermediateTransport::read_from_stream.
pub fn intermediate_frame(input: &[u8]) -> Result<Frame, Error> {
    let header: [u8; 4] = input.get(..4).ok_or(Error::Incomplete)?.try_into().unwrap();
    let length = u32::from_le_bytes(header);
    if length & (1 << 31) != 0 {
        return Ok(Frame::QuickAck(length));
    }
    let length = length as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(Error::Length(length));
    }
    let consumed = length.checked_add(4).ok_or(Error::Length(length))?;
    if input.len() < consumed {
        return Err(Error::Incomplete);
    }
    Ok(Frame::Packet {
        data: 4..consumed,
        consumed,
    })
}
pub fn intermediate_header(length: usize, quick_ack: bool) -> Result<[u8; 4], Error> {
    if length == 0 || length > MAX_PACKET || !length.is_multiple_of(4) {
        return Err(Error::Length(length));
    }
    Ok(((length as u32) | if quick_ack { 1 << 31 } else { 0 }).to_le_bytes())
}

#[derive(Clone, Copy)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}
/// TDLib KDF2; callers retain responsibility for authenticating message keys and padding.
pub fn kdf2(
    auth_key: &[u8; 256],
    msg_key: &[u8; 16],
    direction: Direction,
) -> ([u8; 32], [u8; 32]) {
    let x = match direction {
        Direction::ClientToServer => 0,
        Direction::ServerToClient => 8,
    };
    let mut a = Sha256::new();
    a.update(msg_key);
    a.update(&auth_key[x..x + 36]);
    let a = a.finalize();
    let mut b = Sha256::new();
    b.update(&auth_key[40 + x..76 + x]);
    b.update(msg_key);
    let b = b.finalize();
    let mut key = [0; 32];
    let mut iv = [0; 32];
    key[..8].copy_from_slice(&a[..8]);
    key[8..24].copy_from_slice(&b[8..24]);
    key[24..].copy_from_slice(&a[24..]);
    iv[..8].copy_from_slice(&b[..8]);
    iv[8..24].copy_from_slice(&a[8..24]);
    iv[24..].copy_from_slice(&b[24..]);
    (key, iv)
}
pub fn store_bytes(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    if bytes.len() > MAX_PACKET {
        return Err(Error::Length(bytes.len()));
    }
    let mut result = Vec::with_capacity(bytes.len() + 8);
    if bytes.len() < 254 {
        result.push(bytes.len() as u8);
    } else {
        result.push(254);
        result.extend_from_slice(&(bytes.len() as u32).to_le_bytes()[..3]);
    }
    result.extend_from_slice(bytes);
    result.resize(result.len().next_multiple_of(4), 0);
    Ok(result)
}
pub fn fetch_bytes(input: &[u8], maximum: usize) -> Result<(&[u8], usize), Error> {
    let first = *input.first().ok_or(Error::Incomplete)?;
    let (length, header) = match first {
        0..=253 => (first as usize, 1),
        254 => {
            let bytes = input.get(1..4).ok_or(Error::Incomplete)?;
            (
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]) as usize,
                4,
            )
        }
        // TDLib's internal 255-length extension is not accepted on this network API.
        _ => return Err(Error::Invalid),
    };
    if length > maximum {
        return Err(Error::Length(length));
    }
    let end = header + length;
    let consumed = end.next_multiple_of(4);
    if input.len() < consumed {
        return Err(Error::Incomplete);
    }
    Ok((&input[header..end], consumed))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_and_coalesced_frames_never_expose_incomplete_ranges() {
        let mut packet = intermediate_header(16, false).unwrap().to_vec();
        packet.extend([9; 16]);
        for size in 0..packet.len() {
            assert_eq!(intermediate_frame(&packet[..size]), Err(Error::Incomplete));
        }
        packet.extend_from_within(..);
        assert_eq!(
            intermediate_frame(&packet),
            Ok(Frame::Packet {
                data: 4..20,
                consumed: 20
            })
        );
        assert_eq!(
            intermediate_frame(&0x80000007u32.to_le_bytes()),
            Ok(Frame::QuickAck(0x80000007))
        );
        assert!(matches!(
            intermediate_frame(&0x7fffffffu32.to_le_bytes()),
            Err(Error::Length(_))
        ));
    }
    #[test]
    fn tl_bytes_cover_short_long_boundaries_and_reject_truncation() {
        for length in [0, 1, 3, 4, 253, 254, 255, 1024] {
            let bytes = vec![42; length];
            let encoded = store_bytes(&bytes).unwrap();
            assert_eq!(
                fetch_bytes(&encoded, 2048).unwrap(),
                (bytes.as_slice(), encoded.len())
            );
            for size in 0..encoded.len() {
                assert_eq!(fetch_bytes(&encoded[..size], 2048), Err(Error::Incomplete));
            }
            if length > 0 {
                assert!(fetch_bytes(&encoded, length - 1).is_err());
            }
        }
    }
    #[test]
    fn kdf_matches_independent_vectors_in_both_directions() {
        let auth = std::array::from_fn(|i| i as u8);
        let msg = std::array::from_fn(|i| (i + 10) as u8);
        let vectors = include_str!("kdf-vectors.txt");
        for (line, direction) in vectors
            .lines()
            .zip([Direction::ClientToServer, Direction::ServerToClient])
        {
            let (key, iv) = kdf2(&auth, &msg, direction);
            let actual: String = key
                .iter()
                .chain(iv.iter())
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(actual, line);
        }
    }
}
