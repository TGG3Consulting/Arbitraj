//! Формирование адреса публичного API Native из настроек выбранной площадки.
use super::super::ctx::Ctx;
use crate::gateway::errors::GatewayResult;
use reqwest::Url;

pub(super) const KEY: &str = "native_rfq.base_url";

fn endpoint(raw: &str, method: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.path_segments_mut().ok()?.pop_if_empty().push(method);
    Some(url)
}
pub(super) fn url(ctx: &Ctx, method: &str) -> GatewayResult<Url> {
    endpoint(&ctx.text(KEY)?, method).ok_or_else(|| {
        ctx.not_set(
            KEY,
            "Задайте HTTPS-адрес каталога Native API без параметров, пароля или фрагмента",
        )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn honors_configured_host_prefix_and_escapes_query_values() {
        for s in [
            "https://example.org/prefix/v1",
            "https://example.org/prefix/v1/",
        ] {
            let mut url = endpoint(s, "firm-quote").unwrap();
            url.query_pairs_mut().append_pair("chain", "site&other");
            assert_eq!(
                url.as_str(),
                "https://example.org/prefix/v1/firm-quote?chain=site%26other"
            );
        }
        assert!(endpoint("https://example.org/old?chain=x", "orderbook").is_none());
        assert!(endpoint("https://name:secret@example.org/", "orderbook").is_none());
        assert!(endpoint("http://example.org/", "orderbook").is_none());
    }
}
