use anyhow::{Result, ensure};
pub fn silk_to_wave(input: &[u8]) -> Result<Vec<u8>> {
    let pcm = silk_rs::decode_silk(input, 24000)?;
    pcm_to_wave(&pcm, 24000)
}
pub fn pcm_to_wave(pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>> {
    ensure!(pcm.len().is_multiple_of(2), "PCM 数据不完整");
    let size = u32::try_from(pcm.len())?;
    let mut out = Vec::with_capacity(pcm.len() + 44);
    out.extend(b"RIFF");
    out.extend(
        size.checked_add(36)
            .ok_or_else(|| anyhow::anyhow!("音频过大"))?
            .to_le_bytes(),
    );
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(sample_rate.to_le_bytes());
    out.extend(
        sample_rate
            .checked_mul(2)
            .ok_or_else(|| anyhow::anyhow!("采样率无效"))?
            .to_le_bytes(),
    );
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    out.extend(pcm);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wave_header_matches_pcm() {
        let wav = pcm_to_wave(&[0, 0, 255, 127], 24000).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[44..], &[0, 0, 255, 127]);
        assert!(pcm_to_wave(&[0], 24000).is_err());
    }
}
