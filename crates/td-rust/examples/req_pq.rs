//! Anonymous MTProto transport probe; no Telegram account, API ID or authorization key is used.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "149.154.167.50:443".into())
        .parse()?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|e| format!("{e}"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let message_id =
        ((now.as_secs() << 32) | ((u64::from(now.subsec_nanos()) << 32) / 1_000_000_000)) & !3;
    let mut body = 0xbe7e8ef1u32.to_le_bytes().to_vec();
    body.extend(nonce);
    let mut packet = 0u64.to_le_bytes().to_vec();
    packet.extend(message_id.to_le_bytes());
    packet.extend((body.len() as u32).to_le_bytes());
    packet.extend(body);
    stream.write_all(&td_rust::INTERMEDIATE_TAG)?;
    stream.write_all(&td_rust::intermediate_header(packet.len(), false)?)?;
    stream.write_all(&packet)?;
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if !(40..=4096).contains(&length) {
        return Err(format!("Invalid response size {length}").into());
    }
    let mut reply = vec![0; length];
    stream.read_exact(&mut reply)?;
    if reply[..8] != [0; 8]
        || u32::from_le_bytes(reply[16..20].try_into()?) as usize != reply.len() - 20
        || reply[20..24] != 0x05162463u32.to_le_bytes()
        || reply[24..40] != nonce
    {
        return Err("Invalid resPQ envelope or nonce".into());
    }
    let (pq, used) = td_rust::fetch_bytes(&reply[56..], 64)?;
    let offset = 56 + used;
    if reply.get(offset..offset + 4) != Some(0x1cb5c415u32.to_le_bytes().as_slice()) {
        return Err("Invalid fingerprint vector".into());
    }
    let count = u32::from_le_bytes(
        reply
            .get(offset + 4..offset + 8)
            .ok_or("Missing fingerprints")?
            .try_into()?,
    );
    if reply.len() != offset + 8 + count as usize * 8 {
        return Err("Invalid fingerprint count".into());
    }
    println!(
        "resPQ verified: peer={address}, pq_bytes={}, fingerprints={count}",
        pq.len()
    );
    Ok(())
}
