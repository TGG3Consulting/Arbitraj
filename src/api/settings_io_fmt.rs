//! Файл: api/settings_io_fmt.rs
//! Что делает: формат файла импорта/экспорта настроек (TOML) и перевод значения между формой
//!   хранилища (serde_json::Value) и TOML. Секретов тут нет — только обычные настройки каталога.
//! Кто вызывает: api/settings_io.rs (экспорт и импорт).
//! От чего зависит: toml, serde, serde_json.
//! Настройки: своих нет; работает со всеми настройками каталога по их именам и областям.
//!
//! Формат файла (версия FORMAT_VERSION). Один файл служит обоим режимам:
//!   version = 1
//!   [[setting]]
//!   name  = "money.min_profit_usd"   # имя настройки (docs/SETTINGS.md)
//!   scope = "global"                 # область как в SQLite (Scope::to_key): global, venue:bingx, coin:CAKE, …
//!   value = "5"                      # значение (десятичные — строкой, чтобы не терять точность)
//!
//!   # Необязательная секция — только в «Экспорте с секретами» (полный бэкап):
//!   [secrets]
//!   blob = "…base64…"                # ЗАШИФРОВАННЫЙ файл хранилища секретов КАК ЕСТЬ (docs/SECURITY.md),
//!                                    # base64 только для переноса; открытым текстом секретов тут нет.
//! Без секции [secrets] файл — чистый экспорт настроек (как «Экспорт настроек»).

use serde::{Deserialize, Serialize};

/// Версия формата файла. Схема настроек не вшита — файл просто перечисляет заданные значения.
pub const FORMAT_VERSION: u32 = 1;

/// Версия по умолчанию, если в файле её нет (формат не менялся — разбираем как текущий).
fn default_version() -> u32 {
    FORMAT_VERSION
}

/// Одна запись файла: имя настройки, область (ключ как в SQLite) и значение.
#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub scope: String,
    pub value: toml::Value,
}

/// Секция полного бэкапа: ЗАШИФРОВАННОЕ хранилище секретов как есть (base64). Расшифровки тут нет.
#[derive(Debug, Serialize, Deserialize)]
pub struct Secrets {
    /// base64 от зашифрованного файла секретов (secrets.enc). Только перенос, не расшифровка.
    pub blob: String,
}

/// Весь файл: версия формата, список записей и — в полном бэкапе — зашифрованные секреты.
/// Пустой список настроек допустим (ничего ещё не задано). Секция secrets необязательна.
/// Поля-таблицы (setting, secrets) идут после скалярных (version) — требование формата TOML.
#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub setting: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Secrets>,
}

/// Переводит значение из формы хранилища (JSON) в значение TOML для экспорта.
/// Принимает: значение настройки. Возвращает: значение TOML или причину, почему не представимо
///   (null — не задано; объект — значением настройки не бывает). Очень большое целое — текстом, без потери.
pub fn json_to_toml(v: &serde_json::Value) -> Result<toml::Value, String> {
    use serde_json::Value as J;
    Ok(match v {
        J::String(s) => toml::Value::String(s.clone()),
        J::Bool(b) => toml::Value::Boolean(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        J::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                out.push(json_to_toml(x)?);
            }
            toml::Value::Array(out)
        }
        J::Null => return Err("значение не задано".into()),
        J::Object(_) => return Err("значение-объект настройкой не бывает".into()),
    })
}

/// Переводит значение TOML из файла в форму хранилища (JSON) для проверки и записи через Store::set.
/// Принимает: значение TOML. Возвращает: значение JSON или причину (дата/время, таблица — настройкой
///   не бывают). Проверку по типу настройки делает уже хранилище (settings/value.rs).
pub fn toml_to_json(v: &toml::Value) -> Result<serde_json::Value, String> {
    use toml::Value as T;
    Ok(match v {
        T::String(s) => serde_json::Value::String(s.clone()),
        T::Integer(i) => serde_json::Value::from(*i),
        T::Float(f) => serde_json::Value::from(*f),
        T::Boolean(b) => serde_json::Value::Bool(*b),
        T::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                out.push(toml_to_json(x)?);
            }
            serde_json::Value::Array(out)
        }
        T::Datetime(_) => return Err("дата/время значением настройки не бывает".into()),
        T::Table(_) => return Err("таблица значением настройки не бывает".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Значения всех типов настроек проходят туда-обратно без искажений: строка (десятичное/текст/адрес),
    /// целое, да/нет, список строк. Проверка на настоящих формах значений хранилища, без моков.
    #[test]
    fn value_round_trips_both_ways() {
        let cases = vec![
            json!("5"),                                          // десятичное хранится строкой
            json!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"), // адрес
            json!(1500),                                         // целое (мс)
            json!(true),                                         // да/нет
            json!(["0xAbc", "0xDef"]),                           // список
        ];
        for v in cases {
            let t = json_to_toml(&v).expect("в TOML");
            let back = toml_to_json(&t).expect("обратно в JSON");
            assert_eq!(v, back, "значение исказилось: {v}");
        }
    }

    /// Файл с записями сериализуется в TOML и разбирается обратно один-в-один (версия, имена, области,
    /// значения). Без моков — на собранном вручную файле того же формата, что отдаёт экспорт.
    #[test]
    fn file_serializes_and_parses() {
        let file = File {
            version: FORMAT_VERSION,
            setting: vec![
                Entry {
                    name: "money.min_profit_usd".into(),
                    scope: "global".into(),
                    value: toml::Value::String("5".into()),
                },
                Entry {
                    name: "dex.max_gas_gwei".into(),
                    scope: "dex:pancakeswap_v3".into(),
                    value: toml::Value::Integer(3),
                },
            ],
            secrets: None,
        };
        let text = toml::to_string_pretty(&file).expect("в TOML");
        // Без секции секретов экспорт настроек остаётся чистым TOML настроек.
        assert!(
            !text.contains("[secrets]"),
            "секции секретов быть не должно"
        );
        let parsed: File = toml::from_str(&text).expect("из TOML");
        assert_eq!(parsed.version, FORMAT_VERSION);
        assert_eq!(parsed.setting.len(), 2);
        assert_eq!(parsed.setting[0].name, "money.min_profit_usd");
        assert_eq!(parsed.setting[0].scope, "global");
        assert_eq!(parsed.setting[1].scope, "dex:pancakeswap_v3");
        assert!(parsed.secrets.is_none());
    }

    /// Полный бэкап: секция [secrets] с base64-блобом сериализуется после настроек и разбирается обратно
    /// (блоб — непрозрачный зашифрованный контейнер, тут только переносится).
    #[test]
    fn backup_carries_encrypted_blob() {
        let file = File {
            version: FORMAT_VERSION,
            setting: vec![Entry {
                name: "money.min_profit_usd".into(),
                scope: "global".into(),
                value: toml::Value::String("5".into()),
            }],
            secrets: Some(Secrets {
                blob: "eyJoZWFkZXIiOnt9fQ==".into(),
            }),
        };
        let text = toml::to_string_pretty(&file).expect("в TOML");
        let parsed: File = toml::from_str(&text).expect("из TOML");
        assert_eq!(parsed.setting.len(), 1);
        assert_eq!(
            parsed.secrets.expect("секреты есть").blob,
            "eyJoZWFkZXIiOnt9fQ=="
        );
    }

    /// Версия необязательна: файл без строки version разбирается как текущий формат.
    #[test]
    fn version_defaults_when_absent() {
        let text = "[[setting]]\nname = \"a.b\"\nscope = \"global\"\nvalue = \"1\"\n";
        let parsed: File = toml::from_str(text).expect("из TOML");
        assert_eq!(parsed.version, FORMAT_VERSION);
        assert_eq!(parsed.setting.len(), 1);
    }
}
