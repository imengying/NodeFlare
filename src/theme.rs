use serde_json::Value;

pub fn settings_schema() -> Value {
    let currencies = [
        "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
    ];
    serde_json::json!({
        "schema": 1,
        "source": "builtin",
        "settings": [
            {
                "key": "assetCurrency",
                "label": "资产折算币种",
                "type": "select",
                "default": "CNY",
                "options": currencies.iter().map(|currency| {
                    serde_json::json!({ "label": currency, "value": currency })
                }).collect::<Vec<_>>()
            },
            {
                "key": "enableBlur",
                "label": "启用毛玻璃效果",
                "type": "toggle",
                "default": true
            },
            {
                "key": "showOnline",
                "label": "总览显示在线节点",
                "type": "toggle",
                "default": true
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::settings_schema;

    #[test]
    fn exposes_builtin_glass_theme_settings() {
        let schema = settings_schema();
        let settings = schema["settings"].as_array().expect("settings");
        assert!(settings.iter().any(|field| field["key"] == "assetCurrency"));
        assert!(settings.iter().any(|field| field["key"] == "enableBlur"));
        assert_eq!(schema["source"], "builtin");
    }
}
