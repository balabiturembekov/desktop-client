#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AppKey {
    pub app_name: String,
    pub window_title: String,
    pub url: Option<String>,
}
