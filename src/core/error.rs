//! 统一错误类型。
//!
//! 编解码器、转换管线、服务层与界面层共用同一个错误枚举,便于在界面上统一
//! 展示友好的中文提示。所有解码失败都应返回 [`ConvertError::Corrupt`] 而不是
//! panic,以保证软件在打开损坏文件时不会崩溃。

/// 转换过程中可能出现的错误。
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// 文件头无法匹配任何已注册的格式。
    #[error("无法识别的文件格式")]
    UnknownFormat,

    /// 目标格式不在当前支持列表中。
    #[error("不支持的格式:{0}")]
    UnsupportedFormat(String),

    /// 数据损坏或长度不足。
    #[error("数据损坏:{0}")]
    Corrupt(String),

    /// 格式合法但使用了尚未实现的特性。
    #[error("暂不支持的特性:{0}")]
    UnsupportedFeature(String),

    /// 路径不存在或参数非法。
    #[error("路径无效:{0}")]
    InvalidPath(String),

    /// 底层 IO 失败。
    #[error("文件读写失败:{0}")]
    Io(#[from] std::io::Error),

    /// 配置文件解析失败。
    #[error("配置错误:{0}")]
    Config(String),

    /// JSON 序列化或反序列化失败。
    #[error("数据序列化失败:{0}")]
    Serialize(#[from] serde_json::Error),
}

impl ConvertError {
    /// 构造数据损坏错误。
    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::Corrupt(message.into())
    }

    /// 构造不支持特性错误。
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::UnsupportedFeature(message.into())
    }

    /// 构造路径错误。
    pub fn invalid_path(message: impl Into<String>) -> Self {
        Self::InvalidPath(message.into())
    }

    /// 供界面展示的简短描述。
    pub fn user_message(&self) -> String {
        self.to_string()
    }
}

/// 本项目统一的 `Result` 别名。
pub type Result<T> = std::result::Result<T, ConvertError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_readable() {
        let err = ConvertError::corrupt("文件头长度不足");
        assert_eq!(err.user_message(), "数据损坏:文件头长度不足");

        let err = ConvertError::UnsupportedFormat("webp".to_string());
        assert_eq!(err.user_message(), "不支持的格式:webp");
    }
}
