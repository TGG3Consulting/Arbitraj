//! Файл: venues/binance/sbe_check.rs
//! Что делает: решение 425 — часть кнопки «Проверить подключение» Binance про быстрый стакан SBE: ключ Ed25519
//! (binance.sbe_api_key) не введён или адрес не задан — так и пишется (стакан JSON, как раньше, это не сбой); введён —
//! подключение к stream-sbe с ключом в заголовке: «Подключено» или точная причина отказа (код и тело ответа HTTP).
//! Соединение сразу закрывается, подписок нет.
//! Кто вызывает: mod.rs (check_connection — вместе с key_check.rs).
//! От чего зависит: sbe_run.rs (url, connect), ctx.rs.
//! Настройки: venue.sbe_ws_url [binance], http.timeout_ms.
//!
//! Официально (sbe-market-data-streams.md «General Information»): «An API Key is necessary for access. Only Ed25519
//! keys are allowed. Please put your API Key in the X-MBX-APIKEY header when opening the connection»; «No extra API key
//! permissions are necessary»; «if you use an IP whitelist for the API key, only specified IP addresses are allowed».

use super::ctx::{Ctx, SECRET_SBE_API_KEY};
use super::sbe_run::{connect, url};

/// Что делает: проверка SBE. Возвращает: (итог — false только если ключ введён и подключиться не удалось, текст).
/// Ошибок нет.
pub async fn check(ctx: &Ctx) -> (bool, String) {
    let key = match ctx.vault.get(SECRET_SBE_API_KEY) {
        Ok(k) => k,
        Err(e) => {
            return (
                true,
                format!(
                    "SBE (быстрый стакан): ключ {SECRET_SBE_API_KEY} — {}; стакан JSON, как раньше",
                    e.message
                ),
            );
        }
    };
    let url = match url(ctx) {
        Ok(u) => u,
        Err(why) => {
            return (
                true,
                format!("SBE (быстрый стакан): {why}; стакан JSON, как раньше"),
            );
        }
    };
    let timeout = match ctx.http_timeout() {
        Ok(t) => t,
        Err(e) => return (false, format!("SBE: не проверен — {}", e.message)),
    };
    match connect(&url, &key, timeout).await {
        Ok(mut ws) => {
            let _ = ws.close(None).await;
            (
                true,
                "SBE (быстрый стакан): Подключено — ключ Ed25519 принят".into(),
            )
        }
        Err(why) => (
            false,
            format!("SBE (быстрый стакан): не подключено — {why}"),
        ),
    }
}
