//! SILK 语音转码模块
//!
//! 将微信 SILK 格式语音转码为 WAV 格式

/// 默认采样率（24kHz）
const SILK_SAMPLE_RATE: u32 = 24_000;

/// 将原始 PCM 数据包装为 WAV 格式
/// 
/// 单声道，16位有符号小端序
fn pcm_to_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let pcm_len = pcm.len();
    let total_size = 44 + pcm_len;
    let mut buf = Vec::with_capacity(total_size);

    // RIFF chunk
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(total_size as u32 - 8).to_le_bytes()); // file size - 8
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes());  // PCM format
    buf.extend_from_slice(&1u16.to_le_bytes());  // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate (mono 16-bit)
    buf.extend_from_slice(&2u16.to_le_bytes());  // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(pcm_len as u32).to_le_bytes());
    buf.extend_from_slice(pcm);

    buf
}

/// SILK 转 WAV 结果
#[derive(Debug, Clone)]
pub struct SilkTranscodeResult {
    pub wav_data: Vec<u8>,
    pub duration_ms: u32,
}

/// 尝试将 SILK 音频转码为 WAV
/// 
/// 注意：这需要外部 SILK 解码库支持。由于 Rust 生态中 SILK 解码库有限，
/// 这里提供接口，实际解码可以：
/// 1. 调用外部 silk-wasm 或 silk-decoder 命令
/// 2. 使用 FFI 绑定到 C 库
/// 3. 返回原始 SILK 数据作为回退
pub async fn silk_to_wav(silk_buf: &[u8]) -> Option<SilkTranscodeResult> {
    // 尝试使用外部命令解码
    match decode_with_external_tool(silk_buf).await {
        Some(result) => Some(result),
        None => {
            log::warn!("silk_to_wav: transcode failed, returning raw silk");
            None
        }
    }
}

/// 使用外部工具解码 SILK
async fn decode_with_external_tool(_silk_buf: &[u8]) -> Option<SilkTranscodeResult> {
    // 暂时返回 None，实际实现需要外部 SILK 解码工具
    // 可以集成 silk-wasm 或其他 SILK 解码库
    None
}

#[allow(dead_code)]
async fn decode_with_external_tool_impl(silk_buf: &[u8]) -> Option<SilkTranscodeResult> {
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    // 尝试使用 silk-decoder 或类似的工具
    // 这里提供一个通用接口，实际使用时需要安装相应的解码工具
    
    let temp_dir = std::env::temp_dir();
    let input_path = temp_dir.join(format!("silk_input_{}.silk", std::process::id()));
    let output_path = temp_dir.join(format!("silk_output_{}.pcm", std::process::id()));

    // 写入输入文件
    if let Err(e) = tokio::fs::write(&input_path, silk_buf).await {
        log::warn!("silk_to_wav: failed to write input file: {}", e);
        return None;
    }

    // 尝试 silk-decoder
    let result = Command::new("silk-decoder")
        .arg(&input_path)
        .arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    // 清理输入文件
    let _ = tokio::fs::remove_file(&input_path).await;

    match result {
        Ok(status) if status.success() => {
            // 读取输出文件
            match tokio::fs::read(&output_path).await {
                Ok(pcm_data) => {
                    let _ = tokio::fs::remove_file(&output_path).await;
                    
                    // 计算时长（假设 24kHz 16-bit mono）
                    let duration_ms = (pcm_data.len() as u32 * 1000) / (SILK_SAMPLE_RATE * 2);
                    
                    let wav = pcm_to_wav(&pcm_data, SILK_SAMPLE_RATE);
                    
                    log::debug!(
                        "silk_to_wav: decoded duration={}ms pcmBytes={} wavBytes={}",
                        duration_ms,
                        pcm_data.len(),
                        wav.len()
                    );
                    
                    Some(SilkTranscodeResult {
                        wav_data: wav,
                        duration_ms,
                    })
                }
                Err(e) => {
                    log::warn!("silk_to_wav: failed to read output file: {}", e);
                    let _ = tokio::fs::remove_file(&output_path).await;
                    None
                }
            }
        }
        _ => {
            // 尝试 ffmpeg 作为备选
            decode_with_ffmpeg(silk_buf).await
        }
    }
}

/// 使用 ffmpeg 解码 SILK
async fn decode_with_ffmpeg(silk_buf: &[u8]) -> Option<SilkTranscodeResult> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let silk_buf = silk_buf.to_vec(); // 克隆数据以拥有所有权

    let mut child = Command::new("ffmpeg")
        .args(&[
            "-i", "pipe:0",
            "-f", "s16le",
            "-ar", &SILK_SAMPLE_RATE.to_string(),
            "-ac", "1",
            "-acodec", "pcm_s16le",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    // 写入 SILK 数据
    let write_task = tokio::spawn(async move {
        let _ = stdin.write_all(&silk_buf).await;
        let _ = stdin.shutdown().await;
    });

    // 读取 PCM 输出
    let mut pcm_data = Vec::new();
    let mut stdout = stdout;
    let read_result = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut pcm_data).await;
    
    let _ = write_task.await;
    let status = child.wait().await;

    match (read_result, status) {
        (Ok(_), Ok(status)) if status.success() && !pcm_data.is_empty() => {
            let duration_ms = (pcm_data.len() as u32 * 1000) / (SILK_SAMPLE_RATE * 2);
            let wav = pcm_to_wav(&pcm_data, SILK_SAMPLE_RATE);
            
            log::debug!(
                "silk_to_wav (ffmpeg): duration={}ms pcmBytes={} wavBytes={}",
                duration_ms,
                pcm_data.len(),
                wav.len()
            );
            
            Some(SilkTranscodeResult {
                wav_data: wav,
                duration_ms,
            })
        }
        _ => None,
    }
}

/// 检查是否为 SILK 格式
pub fn is_silk_format(data: &[u8]) -> bool {
    // SILK 文件通常以 "#!SILK_V3" 开头
    data.starts_with(b"#!SILK_V3") || data.starts_with(b"\x02#!SILK_V3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcm_to_wav() {
        let pcm = vec![0u8; 48000]; // 1 second at 24kHz 16-bit mono
        let wav = pcm_to_wav(&pcm, SILK_SAMPLE_RATE);
        
        // WAV header should be at least 44 bytes
        assert_eq!(wav.len(), 44 + pcm.len());
        
        // Check RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        
        // Check fmt chunk
        assert_eq!(&wav[12..16], b"fmt ");
    }

    #[test]
    fn test_is_silk_format() {
        assert!(is_silk_format(b"#!SILK_V3"));
        assert!(is_silk_format(b"\x02#!SILK_V3"));
        assert!(!is_silk_format(b"RIFF"));
        assert!(!is_silk_format(b""));
    }
}
