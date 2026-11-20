//! Файл: venues/bsc_dex/pools/sub_dex.rs
//! Что делает: решение 450 (T448-02) — несколько семейств пулов под одним id индексатора. Под одним id DEX у индексатора
//! (GeckoTerminal) лежат пулы разных фабрик с разным кодом (Curve — StableSwap-NG и прочие шаблоны, Balancer V2 —
//! много фабрик, KyberSwap Classic — DMM и Static Fee), а вид пула (dex.kind) и адреса задаются на ключ области «DEX»
//! (families.rs: одна dex.kind на ключ). Здесь ключ строки справочника получает список дочерних ключей
//! dex.sub_dex — каждый дочерний ключ — обычная DEX этой площадки со своим видом, фабрикой и адресами. Пул строки
//! проверяется видом своего ключа и видом каждого дочернего ключа по порядку списка; вид пула — первый, чья проверка
//! на своём узле (реестр фабрики / хранилища: verify*.rs) пул подтвердила. Проверка каждого вида подтверждает пул только
//! реестром своей фабрики (getPools, isPoolFromFactory, get_coins, getPair …) — пул чужой фабрики не проходит, поэтому
//! вид определяется фабрикой пула, а не догадкой. Ключ строки справочника — по id индексатора (как раньше).
//! Кто вызывает: pools/index_pass.rs (pass), pools/multi_pair.rs (multi_dex).
//! От чего зависит: ctx.rs, ctx_place.rs (own_dex), families.rs, verify.rs (Verdict).
//! Настройки: dex.sub_dex [DEX] (необязательная; не задана — у ключа один вид, как раньше).

use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::super::ctx_place::own_dex;
use super::families::{self, Family};
use super::verify::Verdict;

/// Дочерние ключи DEX `parent` из dex.sub_dex: только ключи этой площадки (тот же суффикс сети), без повторов и без
/// самого `parent`; вложенность не раскрывается (dex.sub_dex дочернего ключа здесь не читается). Принимает: окружение,
/// ключ. Возвращает: список (не задана, не список или сеть площадки не определена — пусто). Ошибок нет.
pub fn keys(ctx: &Ctx, parent: &str) -> Vec<String> {
    let Ok(suffix) = ctx.dex_suffix() else {
        return Vec::new();
    };
    let Ok(list) = ctx.dex_list("dex.sub_dex", parent) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for k in list.iter().map(|k| k.trim()) {
        if !k.is_empty()
            && k != parent
            && own_dex(k, suffix.as_deref())
            && !out.iter().any(|x| x == k)
        {
            out.push(k.to_string());
        }
    }
    out
}

/// Все ключи с заданной dex.sub_dex этой площадки и их дочерние ключи (multi_pair.rs: режим dex.multi_pair у ключа и его
/// дочерних — один). Ошибок нет.
pub fn groups(ctx: &Ctx) -> Vec<(String, Vec<String>)> {
    let Ok(suffix) = ctx.dex_suffix() else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<String>)> = ctx
        .settings
        .all()
        .into_iter()
        .filter(|(name, _, _)| name == "dex.sub_dex")
        .filter_map(|(_, scope, _)| match scope {
            Scope::Dex(id) if own_dex(&id, suffix.as_deref()) => Some(id),
            _ => None,
        })
        .map(|id| {
            let subs = keys(ctx, &id);
            (id, subs)
        })
        .filter(|(_, subs)| !subs.is_empty())
        .collect();
    out.sort();
    out
}

/// Ревью 450 (п. 1): группа ключа — ключ с dex.sub_dex и его дочерние ключи; ключ без группы — он сам. Для дочернего
/// ключа группа — группа его родителя (первого по порядку ключей, в чьём dex.sub_dex он есть). Принимает: окружение,
/// ключ. Возвращает: ключи группы (первый — родитель). Ошибок нет.
pub fn group(ctx: &Ctx, id: &str) -> Vec<String> {
    let own = keys(ctx, id);
    if !own.is_empty() {
        return std::iter::once(id.to_string()).chain(own).collect();
    }
    groups(ctx)
        .into_iter()
        .find(|(_, subs)| subs.iter().any(|k| k == id))
        .map_or_else(
            || vec![id.to_string()],
            |(p, subs)| std::iter::once(p).chain(subs).collect(),
        )
}

/// Виды проверки строки справочника с id `id`: вид самого ключа (если подключён) и подключённые дочерние ключи по
/// порядку dex.sub_dex. Принимает: окружение, ключ. Возвращает: (виды, причины неподключённых — «ключ: причина»).
/// Пустой список видов — DEX не подключена (причина — в списке). Ошибок нет.
///
/// Ревью 450 (п. 3): dex.pool_enabled = нет у ключа строки — дочерние виды не проверяются (строка выключена целиком,
/// причина — та же, что без dex.sub_dex).
pub fn kinds(ctx: &Ctx, id: &str) -> (Vec<&'static Family>, Vec<String>) {
    let mut fams = Vec::new();
    let mut why = Vec::new();
    let subs = match ctx.dex_flag("dex.pool_enabled", id) {
        Ok(false) => Vec::new(),
        _ => keys(ctx, id),
    };
    for k in std::iter::once(id.to_string()).chain(subs) {
        match families::classify(ctx, &k) {
            Ok(f) => fams.push(f),
            Err(w) => why.push(w),
        }
    }
    (fams, why)
}

/// Итог строки по итогам проверки каждым видом (по порядку видов): первый «взят» — взят этим видом; иначе хоть один
/// сбой узла — «не проверен» (повтор); иначе — «отклонён» со всеми причинами «ключ: причина». Принимает: пары (ключ
/// вида, итог). Возвращает: индекс вида-итога (взят — его индекс, иначе 0) и итог. Ошибок нет.
pub fn pick(all: Vec<(&str, Verdict)>) -> (usize, Verdict) {
    if all.len() == 1 {
        let (_, v) = all.into_iter().next().unwrap_or(("", Verdict::Failed(String::new())));
        return (0, v);
    }
    let mut failed = Vec::new();
    let mut rejected = Vec::new();
    for (i, (id, v)) in all.into_iter().enumerate() {
        match v {
            Verdict::Taken(f) => return (i, Verdict::Taken(f)),
            Verdict::Failed(w) => failed.push(format!("{id}: {w}")),
            Verdict::Rejected(w) => rejected.push(format!("{id}: {w}")),
        }
    }
    if !failed.is_empty() {
        return (0, Verdict::Failed(failed.join("; ")));
    }
    (0, Verdict::Rejected(rejected.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Взят второй вид — итог его; ни один не взял, есть сбой узла — «не проверен»; одни отказы — все причины.
    #[test]
    fn pick_first_taken() {
        let r = |w: &str| Verdict::Rejected(w.to_string());
        let (i, v) = pick(vec![("a", r("нет")), ("b", Verdict::Failed("узел".into()))]);
        assert_eq!(i, 0);
        assert!(matches!(v, Verdict::Failed(w) if w == "b: узел"));
        let (_, v) = pick(vec![("a", r("x")), ("b", r("y"))]);
        assert!(matches!(v, Verdict::Rejected(w) if w == "a: x; b: y"));
        let (_, v) = pick(vec![("a", r("x"))]);
        assert!(matches!(v, Verdict::Rejected(w) if w == "x"));
    }
}
