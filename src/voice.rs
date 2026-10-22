use anyhow::{Result, ensure};
pub fn silk_to_wave(input: &[u8]) -> Result<Vec<u8>> {
    let samples = atri_qq::voice::decode_silk(input, 24000)?;
    let pcm: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
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
    #[test]
    fn qq_voice_codec_produces_a_mono_wave_with_the_padded_duration() {
        let samples = 1201usize;
        let pcm: Vec<u8> = (0..samples)
            .flat_map(|i| {
                let value =
                    ((std::f64::consts::TAU * 440. * i as f64 / 24000.).sin() * 6000.) as i16;
                value.to_le_bytes()
            })
            .collect();
        let encoded = atri_qq::voice::encode_silk(&pcm, 24000, 24000, true).unwrap();
        let wav = silk_to_wave(&encoded).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24000);
        assert_eq!(wav.len(), 44 + samples.div_ceil(480) * 480 * 2);
    }
}
