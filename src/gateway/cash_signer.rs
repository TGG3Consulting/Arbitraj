//! Файл: gateway/cash_signer.rs.
//! Назначение: проверяет конкретный публичный API-ключ, которым будет подписан финансовый запрос.
//! Потребители: общий guarded ордер и REST адаптера перед sign::headers, также на резервном URL.
//! Зависимости: SHA-256; имя ключа и секрет в журнал не пишутся.
//! Настройки: отсутствуют.
use super::errors::{ErrorCode, GatewayError, GatewayResult};
use std::future::Future;
tokio::task_local! {static EXPECTED:Option<String>;}
pub async fn scope<T>(expected: String, f: impl Future<Output = T>) -> T {
    let compatible = EXPECTED
        .try_with(|parent| parent.as_ref() == Some(&expected))
        .unwrap_or(true);
    EXPECTED.scope(compatible.then_some(expected), f).await
}
tokio::task_local! {static DISPATCH:std::cell::Cell<bool>;}
pub(crate) fn dispatching() {
    let _ = DISPATCH.try_with(|d| d.set(true));
}
/// Область строго одного POST; до передачи в клиент можно снять собственную подготовленную бронь.
pub(crate) async fn send_order<T>(f: impl Future<Output = GatewayResult<T>>) -> GatewayResult<T> {
    if !crate::core::cash_order::active() {
        return f.await;
    }
    let (r, sent) = DISPATCH
        .scope(std::cell::Cell::new(false), async {
            let r = f.await;
            (r, DISPATCH.with(|d| d.get()))
        })
        .await;
    if r.is_err() && !sent {
        crate::core::cash_order::not_dispatched().await?;
    }
    r
}
pub(crate) fn fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(key.as_bytes()))
}
pub fn verify(key: &str) -> GatewayResult<()> {
    match EXPECTED.try_with(|expected| expected.as_ref() == Some(&fingerprint(key))) {
        Ok(false) => Err(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            "Подписывающий ключ запроса отличается от проверенного счёта",
        )),
        _ => Ok(()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rotation_after_wait_cannot_sign_the_http_request_for_the_other_account() {
        assert!(verify("other").is_ok());
        scope(fingerprint("original"), async {
            assert!(verify("original").is_ok());
            tokio::task::yield_now().await;
            assert!(verify("other").is_err());
            scope(fingerprint("other"), async {
                assert!(verify("other").is_err());
                assert!(verify("original").is_err());
            })
            .await;
        })
        .await;
    }
}
