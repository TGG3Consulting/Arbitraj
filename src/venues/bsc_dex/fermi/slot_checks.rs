//! Проверка актуальности книги строителя перед котировкой и отправкой заявки.
//! Планировщик, лента и исполнитель используют настройки, принятый кадр, родительский
//! блок и ответ соответствующего источника Fermi или PropAMM.
use super::super::parts::Parts;
use super::slot_config::Source;

/// Проверяет поколение настроек, срок, присутствие книги и номер свежей головы.
pub(super) fn timely(p: &Parts, b: &super::markets::Books) -> bool {
    p.ctx.settings.generation() == b.revision
        && b.current()
        && p.slot_markets.contains(b)
        && p.heads
            .fresh(&p.ctx)
            .is_ok_and(|h| h.number == b.head.number.to::<u64>())
}
/// Сверяет привязку контрактов, целевой блок, слот и значения цен двух кадров.
pub(super) fn same_frame(old: &super::Frame, new: &super::Frame) -> bool {
    old.deployment == new.deployment
        && (old.target, old.slot, &old.diff) == (new.target, new.slot, &new.diff)
}
/// Повторно читает голову и кадр выбранного строителя. При ошибке чтения или смене условий возвращает false.
pub(super) async fn fresh(p: &Parts, b: &super::markets::Books) -> bool {
    if !timely(p, b) {
        return false;
    }
    let (head, frame) = tokio::join!(
        super::rpc::call::<super::Head>(
            &p.ctx,
            "eth_getBlockByNumber",
            serde_json::json!(["latest", false])
        ),
        super::builder_snapshot(&p.ctx, b.config.builder)
    );
    let (Ok(h), Ok(raw)) = (head, frame) else {
        return false;
    };
    let new = match &b.config.source {
        Source::Fermi { .. } => super::Frame::read(b.config.builder, &raw, b.config.deployment),
        Source::Prop { oracle, .. } => oracle.frame(b.config.builder, &raw),
    };
    h.hash == b.head.hash && timely(p, b) && new.is_some_and(|f| same_frame(&b.frame, &f))
}
/// Выбирает симуляцию по доказательству заявки. Возвращает расход, выплату и газ либо ошибку симуляции.
pub(super) async fn simulate(
    ctx: &super::Ctx,
    b: &super::markets::Books,
    proof: &super::slot::Proof,
    fees: (u128, u128),
) -> crate::gateway::errors::GatewayResult<(alloy_primitives::U256, alloy_primitives::U256, u64)> {
    if proof.prop.is_some() {
        super::prop_slot::simulate(ctx, &b.head, &b.frame, proof, Some(fees)).await
    } else {
        super::slot::simulate(ctx, &b.head, &b.frame, proof, Some(fees)).await
    }
}
