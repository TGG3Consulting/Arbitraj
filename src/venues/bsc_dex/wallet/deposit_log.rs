//! Индивидуальное зачисление ERC20 из пакетной выплаты. Привязка записывается
//! до выдачи Credited и переживает повтор опроса, смену процесса и гонку двух выводов.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};
use rusqlite::{OptionalExtension, params};

use super::super::{ctx::Ctx, node::abi::IERC20, send::track::Receipt};
use crate::gateway::{contract::DepositClaim, errors::{ErrorCode, GatewayResult}};

pub(super) fn select(
    receipt: &Receipt, token: Address, wallet: Address, hash: B256, expected: Option<U256>,
) -> Result<(u64, U256), &'static str> {
    let mut incoming = Vec::new();
    for log in receipt.logs.iter().filter(|l| l.address() == token) {
        if log.topics().len() != 3 || log.topics()[0] != IERC20::Transfer::SIGNATURE_HASH {
            continue;
        }
        let transfer = IERC20::Transfer::decode_log_data(&log.inner.data)
            .map_err(|_| "Повреждён ERC-20 Transfer в квитанции")?;
        if transfer.to != wallet || transfer.from == wallet || transfer.value.is_zero() {
            continue;
        }
        if log.removed || log.transaction_hash != Some(hash)
            || log.block_number != Some(receipt.block_number.to()) {
            return Err("Лог зачисления не подтверждён этой квитанцией");
        }
        let index = log.log_index.ok_or("В логе зачисления отсутствует logIndex")?;
        if incoming.iter().any(|(seen, _)| *seen == index) {
            return Err("Квитанция содержит повторный logIndex");
        }
        incoming.push((index, transfer.value));
    }
    if let [one] = incoming.as_slice() { return Ok(*one); }
    if incoming.is_empty() { return Err("В квитанции нет поступления этого токена на кошелёк"); }
    let mut matching = incoming.into_iter().filter(|(_, value)| Some(*value) == expected);
    match (matching.next(), matching.next()) {
        (Some(one), None) => Ok(one),
        _ => Err("Пакетный вывод: индивидуальное поступление не определено, нужна сверка"),
    }
}

pub(super) async fn bind(
    ctx: &Ctx, hash: B256, token: Address, wallet: Address, index: u64, raw: U256, claim: &DepositClaim,
) -> GatewayResult<()> {
    let chain = ctx.chain_id().ok_or_else(|| ctx.err(ErrorCode::NotConfigured, "Не задан dex.evm_chain_id для зачисления"))?;
    if claim.withdrawal_id.is_empty() || claim.deal.is_empty() {
        return Err(ctx.err(ErrorCode::DepositNotCredited, "Нет идентификатора вывода для привязки зачисления"));
    }
    let claim = claim.clone();
    let identity = [chain.to_string(),token.to_string(),wallet.to_string(),hash.to_string()];
    let owned = ctx.journal.db().ledger().write(move |c| {
        reserve(c, &identity, index, &raw.to_string(), &claim)
    }).await.map_err(|e| ctx.err(ErrorCode::Internal,format!("Привязка ERC-20 зачисления в учёте: {e}")))?;
    if !owned {
        return Err(ctx.err(ErrorCode::DepositNotCredited, "Лог зачисления или вывод уже закреплён за другой записью; нужна сверка"));
    }
    Ok(())
}

fn reserve(
    c: &mut rusqlite::Connection, [chain,token,wallet,hash]: &[String;4], index: u64, raw: &str,
    owner: &DepositClaim,
) -> rusqlite::Result<bool> {
    let tx = c.transaction()?;
    tx.execute(
        "INSERT INTO deposit_log_claim(chain,token,wallet,tx,log_index,sender,withdrawal_id,deal,raw_amount)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT DO NOTHING",
        params![chain,token,wallet,hash,index,owner.sender.0,owner.withdrawal_id,owner.deal,raw],
    )?;
    let same = tx.query_row(
        "SELECT 1 FROM deposit_log_claim WHERE chain=?1 AND token=?2 AND wallet=?3 AND tx=?4 AND log_index=?5
         AND sender=?6 AND withdrawal_id=?7 AND deal=?8 AND raw_amount=?9",
        params![chain,token,wallet,hash,index,owner.sender.0,owner.withdrawal_id,owner.deal,raw], |_| Ok(()),
    ).optional()?.is_some();
    tx.commit()?;
    Ok(same)
}
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use crate::venues::bsc_dex::num;
    use crate::storage::db::Db;

    fn cases() -> [(Receipt,B256,Address,Address);2] {
        [
            (include_str!("../../../../tests/data/batched_deposit_2026_11_24/tlos.json"),
             "0x55d398326f99059ff775485246999027b3197955"),
            (include_str!("../../../../tests/data/batched_deposit_2026_11_24/num.json"),
             "0xeceb87cf00dcbf2d4e2880223743ff087a995ad9"),
        ].map(|(s,token)|{
            let raw: serde_json::Value=serde_json::from_str(s).unwrap();
            let hash = raw["transactionHash"].as_str().unwrap().parse().unwrap();
            let wallet = "0xdfcbe6dd5056c896066db400f414e2c353c095e9".parse().unwrap();
            (serde_json::from_value(raw).unwrap(), hash, token.parse().unwrap(), wallet)
        })
    }

    #[test]
    fn tlos_and_num_match_individual_logs_and_refuse_ambiguous_credit() {
        for ((mut r,h,token,w),amounts) in cases().into_iter().zip([
            [(143,"61.35492211"),(144,"122.59313556")],
            [(70,"5973.27"),(72,"19366.1271")],
        ]) {
            assert!(select(&r,token,w,h,None).is_err());
            for (index,amount) in amounts {
                let raw=num::to_units(amount.parse().unwrap(),18).unwrap();
                assert_eq!(select(&r,token,w,h,Some(raw)),Ok((index,raw)));
            }
            let chosen=r.logs.iter().find(|l|l.log_index==Some(amounts[0].0)).unwrap().clone();
            r.logs=vec![chosen.clone()];
            assert_eq!(select(&r,token,w,h,None).unwrap().0,amounts[0].0);
            let mut equal=chosen;
            equal.log_index=Some(amounts[1].0);
            r.logs.push(equal);
            assert!(select(&r,token,w,h,Some(num::to_units(amounts[0].1.parse().unwrap(),18).unwrap())).is_err());
        }
    }

    #[tokio::test]
    async fn reservation_survives_restart_and_cannot_switch_withdrawal_or_log() {
        let tmp=std::env::temp_dir().join(format!("arb-log-claim-{}-{}",std::process::id(),chrono::Utc::now().timestamp_micros()));
        std::fs::create_dir(&tmp).unwrap();
        let path=tmp.join("arb.sqlite");
        let claim=|id: &str| DepositClaim {
            sender: crate::core::types::VenueId("kucoin".into()),
            withdrawal_id:id.into(),deal:format!("deal-{id}"),expected:None,
        };
        let key=["56","usdt","w4","tx"].map(str::to_string);
        let check=|db:Db, index, who:DepositClaim, amount:&str|{
            let k=key.clone();let raw=amount.to_owned();
            async move { db.ledger().write(move|conn| reserve(conn,&k,index,&raw,&who)).await.unwrap() }
        };
        let db=Db::open(&path).unwrap();
        assert!(check(db.clone(),143,claim("A"),"61").await);
        drop(db);
        let db=Db::open(&path).unwrap();
        let mut same=claim("A");same.expected=Some(Decimal::ONE);
        assert!(check(db.clone(),143,same,"61").await);
        assert!(!check(db.clone(),143,claim("B"),"61").await);
        assert!(!check(db.clone(),144,claim("A"),"122").await);
        assert!(!check(db.clone(),143,claim("A"),"62").await);
        assert!(check(db,144,claim("B"),"122").await);
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
