//! Файл: gateway/cash_signer.rs.
//! Назначение: проверяет конкретный публичный API-ключ, которым будет подписан финансовый запрос.
//! Потребители: общий guarded ордер и REST адаптера перед sign::headers, также на резервном URL.
//! Зависимости: SHA-256; имя ключа и секрет в журнал не пишутся.
//! Настройки: отсутствуют.
use super::errors::{ErrorCode, GatewayError, GatewayResult};
use std::future::Future;
tokio::task_local! {static EXPECTED:String;}
pub async fn scope<T>(expected: String, f: impl Future<Output = T>) -> T {
    EXPECTED.scope(expected, f).await
}
pub(crate) fn fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(key.as_bytes()))
}
pub fn verify(key: &str) -> GatewayResult<()> {
    match EXPECTED.try_with(|expected| *expected == fingerprint(key)) {
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
        })
        .await;
    }
}
