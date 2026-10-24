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
    fn legacy_multi_frame_packets_keep_every_frame_in_wave_output() {
        let fixtures: &[(&[u8], &[u8])] = &[
            (
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_40ms.silk"),
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_40ms_24000.pcm"),
            ),
            (
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_60ms.silk"),
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_60ms_24000.pcm"),
            ),
            (
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_100ms.silk"),
                include_bytes!("../../atri_qq/silk/tests/fixtures/legacy_16000_100ms_24000.pcm"),
            ),
        ];
        for &(encoded, oracle) in fixtures {
            let wave = silk_to_wave(encoded).unwrap();
            assert_eq!(wave.len(), 44 + oracle.len());
            assert_eq!(
                u32::from_le_bytes(wave[40..44].try_into().unwrap()) as usize,
                oracle.len()
            );
            assert!(wave[44..].iter().any(|byte| *byte != 0));
            assert_eq!(wave, silk_to_wave(&encoded[1..]).unwrap());
        }
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn silk_voice_plays_pauses_and_reaches_end_without_audio_output() {
        use alhazen_core::{
            Player, PlayerConfig, PlayerState, Source,
            audio::{AudioOutputConfig, NullOutput},
        };
        use std::time::{Duration, Instant};
        let pcm = (0..12000)
            .flat_map(|i| {
                let value =
                    ((std::f64::consts::TAU * 440. * i as f64 / 24000.).sin() * 6000.) as i16;
                value.to_le_bytes()
            })
            .collect::<Vec<_>>();
        let silk = atri_qq::voice::encode_silk(&pcm, 24000, 24000, true).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voice.wav");
        std::fs::write(&path, silk_to_wave(&silk).unwrap()).unwrap();
        let output = NullOutput::new(24000, 1);
        let player = Player::open(
            Source::File(path),
            PlayerConfig {
                audio_output: AudioOutputConfig::Null(output.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!player.has_video());
        assert_eq!(player.duration().unwrap(), Duration::from_millis(500));
        player.play();
        let started = Instant::now();
        let mut audible = false;
        while player.position() < Duration::from_millis(80) {
            let _ = player.current_frame();
            audible |= output.pull(240).iter().any(|sample| sample.abs() > 0.001);
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        player.pause();
        let paused = player.position();
        std::thread::sleep(Duration::from_millis(30));
        assert!(player.position().abs_diff(paused) < Duration::from_millis(10));
        assert!(output.pull(240).iter().all(|sample| *sample == 0.));
        player.play();
        while !matches!(player.state(), PlayerState::Ended) {
            let _ = player.current_frame();
            audible |= output.pull(240).iter().any(|sample| sample.abs() > 0.001);
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(audible);
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
