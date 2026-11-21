//! Файл: venues/bsc_dex/exec/dry_judge.rs
//! Что делает: разбор итогов симуляции сухой сделки DEX (решение 121, C4): вход дошёл до кошелька, каждое разрешение и
//! каждый обмен прошли (откат — причина с текстом узла; «срок вышел» — подсказка про dex.deadline_s, как
//! exec/precheck_sim.rs), газ разрешений и обмена, «кто меняет» (решение 59) — check/emitter::missing без исключения
//! пула (сухая сделка справочник не меняет).
//! Кто вызывает: exec/dry_sim.rs.
//! От чего зависит: check/simulate.rs (CallResult, SimLog), check/probe_calls.rs (u256), check/emitter.rs, exec/ur.rs,
//! exec/dry_build.rs (Ready, entries).
//! Настройки: нет.

use alloy_primitives::U256;

use super::super::check::emitter;
use super::super::check::probe_calls as pc;
use super::super::check::simulate::{CallResult, SimLog};
use super::dry_build::{Built, Ready, entries};
use super::dry_sim::Sim;
use super::ur;

/// Разбор итогов: вход дошёл (номер вызова balanceOf входа и сколько нужно), каждый основной вызов успешен, газ, «кто меняет». Отказ — sim.reason. Ошибок нет.
pub fn judge(
    r: &Ready,
    res: &[CallResult],
    mains: &[(usize, bool)],
    input: Option<(usize, U256)>,
    sim: &mut Sim,
) {
    if let Some((i, need)) = input {
        match pc::u256(&res[i]) {
            Some(got) if got >= need => {}
            got => {
                let got = got.map_or("не прочитан".into(), |g| g.to_string());
                sim.reason = Some(format!(
                    "вход у кошелька в симуляции {got} < нужно {need} (налог токена при пополнении?)"
                ));
                return;
            }
        }
    }
    let mut logs: Vec<SimLog> = Vec::new();
    for &(ci, approve) in mains {
        let c = &res[ci];
        if !c.ok() {
            let text = c
                .error
                .as_ref()
                .map(|e| e.message.as_str())
                .unwrap_or_default();
            let what = if approve {
                "разрешение"
            } else {
                "обмен"
            };
            let tail = if ur::deadline_passed(&c.return_data, text) {
                " — TransactionDeadlinePassed: срок вышел к симулированному блоку (время головы + 1 с, решение 127) — проверьте dex.deadline_s и свежесть узла"
            } else {
                ""
            };
            sim.reason = Some(format!("{what} откатился в симуляции: {}{tail}", c.why()));
            return;
        }
        let gas = c.gas_used.to::<u64>();
        if approve {
            sim.gas_approve += gas;
        } else {
            sim.gas_swap += gas;
            logs.extend(c.logs.iter().cloned());
        }
    }
    // Исполнитель (решение 122) вызывает пул ровно по адресу шага — «кто меняет» (решение 59) не проверяется: события
    // обмена его семейств (Solidly, Algebra, DODO…) check/emitter.rs не знает.
    if matches!(r.built, Built::Exec(_)) {
        return;
    }
    let pools = entries(&r.legs);
    let bad = emitter::missing(&pools, &logs);
    if !bad.is_empty() {
        let list: Vec<String> = bad.into_iter().map(|(_, w)| w).collect();
        sim.reason = Some(format!(
            "обмен сделал не пул маршрута (решение 59): {}",
            list.join("; ")
        ));
    }
}
