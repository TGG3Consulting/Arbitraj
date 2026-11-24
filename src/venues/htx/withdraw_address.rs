/*! Файл: venues/htx/withdraw_address.rs
Что делает: проверяет перед выводом адрес монеты и общий адрес той же сети. Область
выбирает API HTX: для общих адресов currency=t247117, chain сети (для BSC — bep20).
Кто вызывает: transfers.rs. От чего зависит: rest.rs, networks_parse.rs, records.rs.
Документация: https://www.htx.com/en-us/opend/newApiPages/?id=7ec50654-7773-11ed-9966-0242ac110003
*/
use std::collections::HashSet;
use std::future::Future;

use serde_json::Value;

use super::ctx::{Ctx, currency};
use super::networks_parse::TransferChain;
use super::{errors, records, rest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

const BOOK: &str = "/v2/account/withdraw/address";
const UNIVERSAL: &str = "t247117";

/// Проверяет назначение перед POST. Ошибка чтения любой нужной страницы останавливает вывод.
/// WithdrawRejected — обе применимые области просмотрены и подходящей записи нет.
pub(super) async fn check(
    ctx: &Ctx,
    ticker: &str,
    chain: &TransferChain,
    address: &str,
) -> GatewayResult<()> {
    let coin = currency(ticker);
    if listed(&coin, chain, address, |q: AddressPage| {
        rest::page(ctx, BOOK, q.params())
    })
    .await?
    {
        return Ok(());
    }
    Err(ctx.tag(GatewayError::new(
        ErrorCode::WithdrawRejected,
        None,
        format!(
            "В адресах вывода текущего API-ключа HTX нет подходящей записи для {address} ({ticker}, chain {})",
            chain.coin
        ),
    )))
}

/// Параметры страницы адресной книги HTX.
struct AddressPage {
    currency: String,
    chain: String,
    from: Option<u64>,
}

impl AddressPage {
    fn params(self) -> rest::Params {
        let mut params = vec![
            ("currency", self.currency),
            ("chain", self.chain),
            ("limit", records::SIZE_MAX.to_string()),
        ];
        if let Some(id) = self.from {
            params.push(("fromId", id.to_string()));
        }
        params
    }
}

/// Читает сначала книгу монеты, затем общую книгу выбранной сети. Возвращает false только после
/// последней страницы. Курсор HTX непрозрачный, порядок его значений не задан.
async fn listed<F>(
    currency: &str,
    network: &TransferChain,
    address: &str,
    mut read: impl FnMut(AddressPage) -> F + Send,
) -> GatewayResult<bool>
where
    F: Future<Output = GatewayResult<Value>> + Send,
{
    let individual = (currency, network.coin.as_str());
    let universal = network.universal.map(|n| (UNIVERSAL, n));
    for (coin, chain) in std::iter::once(individual).chain(universal) {
        let mut from: Option<u64> = None;
        let mut seen = HashSet::new();
        loop {
            let page = read(AddressPage {
                currency: coin.to_string(),
                chain: chain.to_string(),
                from,
            })
            .await?;
            let rows = page["data"]
                .as_array()
                .ok_or_else(|| errors::bad("адреса вывода: data не массив", ""))?;
            if rows.iter().any(|row| matches(row, coin, chain, address)) {
                return Ok(true);
            }
            let Some(next) = cursor(&page)? else {
                break;
            };
            if !seen.insert(next) {
                return Err(errors::bad("адреса вывода: повтор курсора страниц", ""));
            }
            from = Some(next);
        }
    }
    Ok(false)
}

/// Схема HTX называет курсор nextId, в примере он next-id. Ошибка BadResponse — нечитаемый курсор.
fn cursor(page: &Value) -> GatewayResult<Option<u64>> {
    page.get("nextId")
        .or_else(|| page.get("next-id"))
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| errors::bad("адреса вывода: неверный курсор страницы", ""))
        })
        .transpose()
}

/// Для обычной записи сохраняет прежнюю проверку адреса в отфильтрованной книге монеты.
/// Общая запись относится к конкретной сети, а текущий POST не передаёт addressTag.
/// Статус белого списка сайта не определяет доступ API (https://www.htx.com/support/54883603954255/).
fn matches(row: &Value, coin: &str, chain: &str, address: &str) -> bool {
    let same = |key: &str, expected: &str| {
        row[key]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(expected))
    };
    if !same("address", address)
        || !(row["addressTag"].is_null() || row["addressTag"].as_str() == Some(""))
    {
        return false;
    }
    if coin == UNIVERSAL {
        return same("currency", UNIVERSAL) && same("chain", chain);
    }
    // У старого индивидуального ответа бывает только адрес; GET уже ограничен валютой и сетью.
    // Если HTX повторил поля строки, они должны соответствовать запросу.
    (row.get("currency").is_none() || same("currency", coin))
        && (row.get("chain").is_none() || same("chain", chain))
}

#[cfg(test)]
mod tests {
    use super::super::networks_parse::universal_chain;
    use super::*;
    use serde_json::json;

    // Адрес публичного примера HTX заменяет адрес пользователя. Остальные значимые поля —
    // обезличенная выборка двух сетевых записей действующего ключа, 23.11.2026.
    // Отсутствие addressTag моделирует необязательное поле спецификации HTX.
    const ADDRESS: &str = "0x3b994f25c4c25e99d4d26364ffc014cce64600ca";

    /// Порядок запросов на сделке XTER. Пустой индивидуальный ответ получен от HTX, курсор взят из примера;
    /// общая запись обезличена. Повтор чтения не отправляет реальный запрос или вывод.
    #[tokio::test]
    async fn reads_individual_then_matching_universal_book_and_follows_its_cursor() {
        let network = TransferChain {
            coin: "xter".into(),
            universal: universal_chain(&json!({"displayName":"BEP20"})),
        };
        let mut pages = [
            ("xter", "xter", None, json!({"code":200,"data":[]})),
            (
                "t247117",
                "bep20",
                None,
                json!({"code":200,"data":[{"currency":"t247117","chain":"bep20","address":"0xde709f2102306220921060314715629080e2fb77"}],"next-id":30137790}),
            ),
            (
                "t247117",
                "bep20",
                Some("30137790"),
                json!({"code":200,"data":[{
                    "currency":"t247117", "chain":"bep20", "address":ADDRESS, "status":"normal"
                }]}),
            ),
        ]
        .into_iter();
        let found = listed("xter", &network, ADDRESS, |page: AddressPage| {
            let params = page.params();
            let (coin, chain, from, response) = pages.next().expect("лишний запрос");
            let get = |name| {
                params
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.as_str())
            };
            assert_eq!(get("currency"), Some(coin));
            assert_eq!(get("chain"), Some(chain));
            assert_eq!(get("fromId"), from);
            std::future::ready(Ok(response))
        })
        .await
        .unwrap();
        assert!(found && pages.next().is_none());
    }

    /// Пустая книга монеты и общая запись того же адреса только на Arbitrum не разрешают вывод XTER на BSC.
    #[tokio::test]
    async fn wrong_network_is_rejected_after_both_scopes() {
        let network = TransferChain {
            coin: "xter".into(),
            universal: Some("bep20"),
        };
        let mut responses = [
            json!({"code":200,"data":[]}),
            json!({"code":200,"data":[{
                "currency":"t247117","chain":"arbitrum","address":ADDRESS,"status":"normal"
            }]}),
        ]
        .into_iter();
        let found = listed("xter", &network, ADDRESS, |_| {
            std::future::ready(Ok(responses.next().expect("лишний запрос")))
        })
        .await
        .unwrap();
        assert!(!found && responses.next().is_none());
    }

    /// Совпадение в книге монеты прекращает поиск на первой странице.
    #[tokio::test]
    async fn individual_match_skips_universal_book() {
        let network = TransferChain {
            coin: "bep20usdt".into(),
            universal: Some("bep20"),
        };
        assert!(
            listed("usdt", &network, ADDRESS, |page: AddressPage| {
                let params = page.params();
                assert!(params.contains(&("currency", "usdt".into())));
                std::future::ready(Ok(json!({"code":200, "data":[{"address":ADDRESS}]})))
            })
            .await
            .unwrap()
        );
    }

    #[test]
    fn universal_records_apply_only_to_the_selected_network() {
        for (coin_chain, network) in [("xter", "BEP20"), ("arb", "ARBITRUM")] {
            let info = json!({"chain":coin_chain,"displayName":network});
            let selected = universal_chain(&info).unwrap();
            let row = json!({"currency":"t247117","chain":network.to_lowercase(),"address":ADDRESS,"status":"normal"});
            assert!(matches(&row, UNIVERSAL, selected, &ADDRESS.to_uppercase()));
            assert!(!matches(&row, UNIVERSAL, coin_chain, ADDRESS));
            let other = if network == "BEP20" {
                "ARBITRUM"
            } else {
                "bep20"
            };
            assert!(!matches(&row, UNIVERSAL, other, ADDRESS));
        }
        assert_eq!(universal_chain(&json!({"displayName":"XRP"})), None);
    }

    #[test]
    fn universal_match_requires_currency_and_no_tag() {
        let mut row =
            json!({"currency":"t247117","chain":"bep20","address":ADDRESS,"status":"white"});
        assert!(matches(&row, UNIVERSAL, "bep20", ADDRESS));
        row["addressTag"] = json!("123");
        assert!(!matches(&row, UNIVERSAL, "bep20", ADDRESS));
        row["addressTag"] = json!("");
        row["status"] = json!("white-delaying");
        assert!(matches(&row, UNIVERSAL, "bep20", ADDRESS));
        row.as_object_mut().unwrap().remove("status");
        assert!(matches(&row, UNIVERSAL, "bep20", ADDRESS));
        row["currency"] = json!("usdt");
        assert!(!matches(&row, UNIVERSAL, "bep20", ADDRESS));
    }

    #[test]
    fn individual_address_and_pagination_from_htx_example() {
        let page = json!({"code":200,"data":[{"currency":"usdt","chain":"hrc20usdt","note":"tom","addressTag":"","address":ADDRESS}],"next-id":30137790});
        assert!(matches(&page["data"][0], "usdt", "hrc20usdt", ADDRESS));
        assert_eq!(cursor(&page).unwrap(), Some(30137790));
        assert_eq!(cursor(&json!({"nextId":30137790})).unwrap(), Some(30137790));
        assert_eq!(cursor(&json!({"data":[]})).unwrap(), None);
        assert!(cursor(&json!({"next-id":{}})).is_err());
    }
}
