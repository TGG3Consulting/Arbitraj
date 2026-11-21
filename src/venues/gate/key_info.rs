//! Файл: venues/gate/key_info.rs
//! Что делает: из ответа GET /account/main_keys выбирает запись ключа, которым работает бот.
//! Кто вызывает: account.rs (кнопка «Проверить подключение»), тесты.
//! От чего зависит: только serde_json.
//! Настройки: нет.
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ — «Query All Main Account Key Information»
//! (GET /account/main_keys): ответ — СПИСОК ключей основного аккаунта (AccountKeyInfo[]: key, state, perms, ip_whitelist…),
//! а не одна запись. Живой ответ 04.11.2026 (сырой журнал бота, ~/first-bsc/data/raw/gate/) — массив из 4 ключей; прежняя
//! проверка читала поля верхнего уровня как у одной записи, не находила perms и писала «нет права спота» у ключа, где
//! спот на запись включён (решение 135, дополнение 04.11).

use serde_json::Value;

/// Запись своего ключа. Принимает: ответ /account/main_keys, свой ключ (gate.api_key). Возвращает: запись, у которой
/// поле key совпадает со своим ключом; ответ-объект (одна запись) — он сам, если key нет или совпадает. Ничего не нашлось
/// — None (ключа бота в списке нет — причина в «Проверить подключение»). Ошибок нет.
pub fn own<'a>(keys: &'a Value, my_key: &str) -> Option<&'a Value> {
    let matches = |v: &Value| v.get("key").and_then(Value::as_str) == Some(my_key);
    match keys {
        Value::Array(a) => a.iter().find(|v| matches(v)),
        Value::Object(_) => match keys.get("key") {
            None => Some(keys),
            Some(_) if matches(keys) => Some(keys),
            Some(_) => None,
        },
        _ => None,
    }
}
