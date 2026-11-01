//! MIME 类型处理模块
//!
//! 根据文件名或内容类型确定 MIME 类型和扩展名

use std::collections::HashMap;
use once_cell::sync::Lazy;

/// MIME 类型映射表
static MIME_MAP: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut map = HashMap::new();
    
    // 图片
    map.insert("jpg", "image/jpeg");
    map.insert("jpeg", "image/jpeg");
    map.insert("png", "image/png");
    map.insert("gif", "image/gif");
    map.insert("bmp", "image/bmp");
    map.insert("webp", "image/webp");
    map.insert("svg", "image/svg+xml");
    
    // 视频
    map.insert("mp4", "video/mp4");
    map.insert("avi", "video/x-msvideo");
    map.insert("mov", "video/quicktime");
    map.insert("wmv", "video/x-ms-wmv");
    map.insert("flv", "video/x-flv");
    map.insert("mkv", "video/x-matroska");
    map.insert("webm", "video/webm");
    
    // 音频
    map.insert("mp3", "audio/mpeg");
    map.insert("wav", "audio/wav");
    map.insert("ogg", "audio/ogg");
    map.insert("m4a", "audio/mp4");
    map.insert("aac", "audio/aac");
    map.insert("flac", "audio/flac");
    map.insert("silk", "audio/silk");
    
    // 文档
    map.insert("pdf", "application/pdf");
    map.insert("doc", "application/msword");
    map.insert("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    map.insert("xls", "application/vnd.ms-excel");
    map.insert("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
    map.insert("ppt", "application/vnd.ms-powerpoint");
    map.insert("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation");
    map.insert("txt", "text/plain");
    map.insert("csv", "text/csv");
    map.insert("rtf", "application/rtf");
    
    // 压缩文件
    map.insert("zip", "application/zip");
    map.insert("rar", "application/x-rar-compressed");
    map.insert("7z", "application/x-7z-compressed");
    map.insert("tar", "application/x-tar");
    map.insert("gz", "application/gzip");
    map.insert("bz2", "application/x-bzip2");
    
    // 代码文件
    map.insert("html", "text/html");
    map.insert("htm", "text/html");
    map.insert("css", "text/css");
    map.insert("js", "application/javascript");
    map.insert("json", "application/json");
    map.insert("xml", "application/xml");
    map.insert("yaml", "application/yaml");
    map.insert("yml", "application/yaml");
    
    map
});

/// 扩展名映射表（MIME -> 扩展名）
static EXT_MAP: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut map = HashMap::new();
    
    map.insert("image/jpeg", "jpg");
    map.insert("image/png", "png");
    map.insert("image/gif", "gif");
    map.insert("image/bmp", "bmp");
    map.insert("image/webp", "webp");
    map.insert("image/svg+xml", "svg");
    
    map.insert("video/mp4", "mp4");
    map.insert("video/x-msvideo", "avi");
    map.insert("video/quicktime", "mov");
    map.insert("video/x-ms-wmv", "wmv");
    map.insert("video/webm", "webm");
    
    map.insert("audio/mpeg", "mp3");
    map.insert("audio/wav", "wav");
    map.insert("audio/ogg", "ogg");
    map.insert("audio/mp4", "m4a");
    map.insert("audio/aac", "aac");
    map.insert("audio/flac", "flac");
    map.insert("audio/silk", "silk");
    
    map.insert("application/pdf", "pdf");
    map.insert("application/msword", "doc");
    map.insert("application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx");
    map.insert("application/vnd.ms-excel", "xls");
    map.insert("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "xlsx");
    map.insert("application/vnd.ms-powerpoint", "ppt");
    map.insert("application/vnd.openxmlformats-officedocument.presentationml.presentation", "pptx");
    map.insert("text/plain", "txt");
    map.insert("text/csv", "csv");
    map.insert("application/rtf", "rtf");
    
    map.insert("application/zip", "zip");
    map.insert("application/x-rar-compressed", "rar");
    map.insert("application/x-7z-compressed", "7z");
    map.insert("application/x-tar", "tar");
    map.insert("application/gzip", "gz");
    
    map.insert("text/html", "html");
    map.insert("text/css", "css");
    map.insert("application/javascript", "js");
    map.insert("application/json", "json");
    map.insert("application/xml", "xml");
    map.insert("application/yaml", "yaml");
    
    map
});

/// 根据文件名获取 MIME 类型
pub fn get_mime_from_filename(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    
    MIME_MAP
        .get(ext.as_str())
        .map(|&s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// 根据 MIME 类型获取扩展名
pub fn get_extension_from_mime(mime_type: &str) -> String {
    EXT_MAP
        .get(mime_type)
        .map(|&s| s.to_string())
        .unwrap_or_else(|| "bin".to_string())
}

/// 根据 Content-Type 或 URL 获取扩展名
pub fn get_extension_from_content_type_or_url(
    content_type: Option<&str>,
    url: &str,
) -> String {
    // 首先尝试从 Content-Type 获取
    if let Some(ct) = content_type {
        // 移除 charset 等参数
        let mime = ct.split(';').next().unwrap_or(ct).trim();
        let ext = get_extension_from_mime(mime);
        if ext != "bin" {
            return ext;
        }
    }
    
    // 从 URL 路径获取
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(path) = parsed.path_segments() {
            if let Some(last) = path.last() {
                let ext = last
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                if !ext.is_empty() {
                    return ext;
                }
            }
        }
    }
    
    "bin".to_string()
}

/// 判断是否为图片类型
pub fn is_image_mime(mime_type: &str) -> bool {
    mime_type.starts_with("image/")
}

/// 判断是否为视频类型
pub fn is_video_mime(mime_type: &str) -> bool {
    mime_type.starts_with("video/")
}

/// 判断是否为音频类型
pub fn is_audio_mime(mime_type: &str) -> bool {
    mime_type.starts_with("audio/")
}

/// 判断是否为文档类型
pub fn is_document_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/pdf"
            | "application/msword"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.ms-powerpoint"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "text/plain"
            | "text/csv"
            | "application/rtf"
    )
}

/// 获取文件类型的通用分类
pub fn get_file_category(mime_type: &str) -> &'static str {
    if is_image_mime(mime_type) {
        "image"
    } else if is_video_mime(mime_type) {
        "video"
    } else if is_audio_mime(mime_type) {
        "audio"
    } else if is_document_mime(mime_type) {
        "document"
    } else if mime_type.contains("zip") || mime_type.contains("compressed") {
        "archive"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_mime_from_filename() {
        assert_eq!(get_mime_from_filename("photo.jpg"), "image/jpeg");
        assert_eq!(get_mime_from_filename("document.PDF"), "application/pdf");
        assert_eq!(get_mime_from_filename("video.mp4"), "video/mp4");
        assert_eq!(get_mime_from_filename("unknown.xyz"), "application/octet-stream");
    }

    #[test]
    fn test_get_extension_from_mime() {
        assert_eq!(get_extension_from_mime("image/jpeg"), "jpg");
        assert_eq!(get_extension_from_mime("application/pdf"), "pdf");
        assert_eq!(get_extension_from_mime("video/mp4"), "mp4");
        assert_eq!(get_extension_from_mime("unknown/type"), "bin");
    }

    #[test]
    fn test_get_extension_from_content_type_or_url() {
        // 从 Content-Type
        assert_eq!(
            get_extension_from_content_type_or_url(Some("image/jpeg"), "http://example.com/file"),
            "jpg"
        );
        
        // 从 URL
        assert_eq!(
            get_extension_from_content_type_or_url(None, "http://example.com/photo.png"),
            "png"
        );
        
        // 默认
        assert_eq!(
            get_extension_from_content_type_or_url(None, "http://example.com/file"),
            "bin"
        );
    }

    #[test]
    fn test_is_image_mime() {
        assert!(is_image_mime("image/jpeg"));
        assert!(is_image_mime("image/png"));
        assert!(!is_image_mime("video/mp4"));
    }

    #[test]
    fn test_get_file_category() {
        assert_eq!(get_file_category("image/jpeg"), "image");
        assert_eq!(get_file_category("video/mp4"), "video");
        assert_eq!(get_file_category("audio/mp3"), "audio");
        assert_eq!(get_file_category("application/pdf"), "document");
        assert_eq!(get_file_category("application/zip"), "archive");
        assert_eq!(get_file_category("unknown/type"), "other");
    }
}
