//! 媒体标签解析模块
//! 支持 <image>, <voice>, <video>, <file> 标签

use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

use crate::api::QQBotApi;
use crate::types::*;

/// 媒体标签类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaTagType {
    Image,
    Voice,
    Video,
    File,
}

impl MediaTagType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaTagType::Image => "image",
            MediaTagType::Voice => "voice",
            MediaTagType::Video => "video",
            MediaTagType::File => "file",
        }
    }

    pub fn to_media_file_type(&self) -> MediaFileType {
        match self {
            MediaTagType::Image => MediaFileType::Image,
            MediaTagType::Voice => MediaFileType::Voice,
            MediaTagType::Video => MediaFileType::Video,
            MediaTagType::File => MediaFileType::File,
        }
    }
}

/// 解析出的媒体标签
#[derive(Debug, Clone)]
pub struct MediaTag {
    pub tag_type: MediaTagType,
    pub src: String,
    pub caption: Option<String>,
    pub raw: String,
}

/// 媒体标签解析器
pub struct MediaTagParser;

impl MediaTagParser {
    /// 解析文本中的媒体标签
    pub fn parse(text: &str) -> (String, Vec<MediaTag>) {
        let mut media_tags = Vec::new();
        let mut result_text = text.to_string();

        // 解析 <image src="..."> 或 <image>url</image>
        let image_regex = get_image_regex();
        for cap in image_regex.captures_iter(text) {
            let raw = cap.get(0).unwrap().as_str();
            let src = cap
                .name("src")
                .map(|m| m.as_str().to_string())
                .or_else(|| cap.name("url").map(|m| m.as_str().to_string()))
                .unwrap_or_default();
            let caption = cap.name("caption").map(|m| m.as_str().to_string());

            if !src.is_empty() {
                media_tags.push(MediaTag {
                    tag_type: MediaTagType::Image,
                    src,
                    caption,
                    raw: raw.to_string(),
                });
                // 从文本中移除标签
                result_text = result_text.replace(raw, "");
            }
        }

        // 解析 <voice src="..."> 或 <voice>url</voice>
        let voice_regex = get_voice_regex();
        for cap in voice_regex.captures_iter(text) {
            let raw = cap.get(0).unwrap().as_str();
            let src = cap
                .name("src")
                .map(|m| m.as_str().to_string())
                .or_else(|| cap.name("url").map(|m| m.as_str().to_string()))
                .unwrap_or_default();

            if !src.is_empty() {
                media_tags.push(MediaTag {
                    tag_type: MediaTagType::Voice,
                    src,
                    caption: None,
                    raw: raw.to_string(),
                });
                result_text = result_text.replace(raw, "");
            }
        }

        // 解析 <video src="..."> 或 <video>url</video>
        let video_regex = get_video_regex();
        for cap in video_regex.captures_iter(text) {
            let raw = cap.get(0).unwrap().as_str();
            let src = cap
                .name("src")
                .map(|m| m.as_str().to_string())
                .or_else(|| cap.name("url").map(|m| m.as_str().to_string()))
                .unwrap_or_default();
            let caption = cap.name("caption").map(|m| m.as_str().to_string());

            if !src.is_empty() {
                media_tags.push(MediaTag {
                    tag_type: MediaTagType::Video,
                    src,
                    caption,
                    raw: raw.to_string(),
                });
                result_text = result_text.replace(raw, "");
            }
        }

        // 解析 <file src="..."> 或 <file>url</file>
        let file_regex = get_file_regex();
        for cap in file_regex.captures_iter(text) {
            let raw = cap.get(0).unwrap().as_str();
            let src = cap
                .name("src")
                .map(|m| m.as_str().to_string())
                .or_else(|| cap.name("url").map(|m| m.as_str().to_string()))
                .unwrap_or_default();
            let caption = cap.name("caption").map(|m| m.as_str().to_string());

            if !src.is_empty() {
                media_tags.push(MediaTag {
                    tag_type: MediaTagType::File,
                    src,
                    caption,
                    raw: raw.to_string(),
                });
                result_text = result_text.replace(raw, "");
            }
        }

        // 清理多余空白
        result_text = result_text.split_whitespace().collect::<Vec<_>>().join(" ");

        (result_text, media_tags)
    }

    /// 检查文本是否包含媒体标签
    pub fn has_media_tags(text: &str) -> bool {
        let (_, tags) = Self::parse(text);
        !tags.is_empty()
    }
}

/// 媒体标签处理器
pub struct MediaTagHandler<'a> {
    api: &'a QQBotApi,
    target_id: &'a str,
    target_type: TargetType,
}

impl<'a> MediaTagHandler<'a> {
    pub fn new(api: &'a QQBotApi, target_id: &'a str, target_type: TargetType) -> Self {
        Self {
            api,
            target_id,
            target_type,
        }
    }

    /// 处理并发送媒体标签
    pub async fn handle_media_tags(&self, text: &str) -> Result<(String, Vec<MediaSendResult>)> {
        let (clean_text, media_tags) = MediaTagParser::parse(text);
        let mut results = Vec::new();

        for tag in &media_tags {
            let result = self.send_media_tag(tag).await?;
            results.push(result);
        }

        Ok((clean_text, results))
    }

    /// 发送单个媒体标签
    async fn send_media_tag(&self, tag: &MediaTag) -> Result<MediaSendResult> {
        let file_type = tag.tag_type.to_media_file_type();
        let options = SendOptions::default();

        // 获取文件数据
        let (file_data, is_url) = if tag.src.starts_with("http://") || tag.src.starts_with("https://") {
            (tag.src.clone(), true)
        } else if tag.src.starts_with("data:") {
            // Base64 数据
            let parts: Vec<&str> = tag.src.splitn(2, ',').collect();
            if parts.len() == 2 {
                (parts[1].to_string(), false)
            } else {
                return Err(anyhow::anyhow!("Invalid base64 data URL"));
            }
        } else {
            // 本地文件
            let content = tokio::fs::read(&tag.src).await?;
            (base64::encode(&content), false)
        };

        // 上传并发送
        let upload_result = match self.target_type {
            TargetType::C2C => {
                if is_url && !tag.src.starts_with("data:") {
                    self.api.upload_c2c_media(
                        self.target_id,
                        file_type,
                        Some(&file_data),
                        None,
                        tag.caption.as_deref(),
                    ).await?
                } else {
                    self.api.upload_c2c_media(
                        self.target_id,
                        file_type,
                        None,
                        Some(&file_data),
                        tag.caption.as_deref(),
                    ).await?
                }
            }
            TargetType::Group => {
                if is_url && !tag.src.starts_with("data:") {
                    self.api.upload_group_media(
                        self.target_id,
                        file_type,
                        Some(&file_data),
                        None,
                        tag.caption.as_deref(),
                    ).await?
                } else {
                    self.api.upload_group_media(
                        self.target_id,
                        file_type,
                        None,
                        Some(&file_data),
                        tag.caption.as_deref(),
                    ).await?
                }
            }
            TargetType::Channel => {
                return Err(anyhow::anyhow!("Channel media not supported"));
            }
        };

        // 发送媒体消息
        let msg_result = match self.target_type {
            TargetType::C2C => {
                self.api.send_c2c_media_message(
                    self.target_id,
                    &upload_result.file_info,
                    tag.caption.as_deref(),
                    options,
                ).await?
            }
            TargetType::Group => {
                self.api.send_group_media_message(
                    self.target_id,
                    &upload_result.file_info,
                    tag.caption.as_deref(),
                    options,
                ).await?
            }
            TargetType::Channel => {
                return Err(anyhow::anyhow!("Channel media not supported"));
            }
        };

        Ok(MediaSendResult {
            tag_type: tag.tag_type.clone(),
            message_id: msg_result.id,
            success: true,
        })
    }
}

/// 媒体发送结果
#[derive(Debug, Clone)]
pub struct MediaSendResult {
    pub tag_type: MediaTagType,
    pub message_id: String,
    pub success: bool,
}

// 正则表达式缓存
fn get_image_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"<image\s+(?:src=["'](?P<src>[^"']+)["']|caption=["'](?P<caption>[^"']+)["'])\s*/?>|<image>(?P<url>[^<]+)</image>"#
        ).unwrap()
    })
}

fn get_voice_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"<voice\s+(?:src=["'](?P<src>[^"']+)["'])\s*/?>|<voice>(?P<url>[^<]+)</voice>"#
        ).unwrap()
    })
}

fn get_video_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"<video\s+(?:src=["'](?P<src>[^"']+)["']|caption=["'](?P<caption>[^"']+)["'])\s*/?>|<video>(?P<url>[^<]+)</video>"#
        ).unwrap()
    })
}

fn get_file_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"<file\s+(?:src=["'](?P<src>[^"']+)["']|caption=["'](?P<caption>[^"']+)["'])\s*/?>|<file>(?P<url>[^<]+)</file>"#
        ).unwrap()
    })
}

// Base64 编码辅助函数
mod base64 {
    pub fn encode(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_tag() {
        let text = r#"Hello <image src="https://example.com/img.png" /> world"#;
        let (clean, tags) = MediaTagParser::parse(text);
        assert_eq!(clean, "Hello world");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag_type, MediaTagType::Image);
        assert_eq!(tags[0].src, "https://example.com/img.png");
    }

    #[test]
    fn test_parse_multiple_tags() {
        let text = r#"<image>https://a.com/1.png</image> text <voice>https://b.com/v.mp3</voice>"#;
        let (clean, tags) = MediaTagParser::parse(text);
        assert_eq!(clean, "text");
        assert_eq!(tags.len(), 2);
    }
}
