"""Файл: tools/arbitrum_record/v4live_cases.py
Что делает: решение 459 (T457-02) — вызовы приёмки исполнителя v4 по правилу решения 400 (формат строк summary.json —
  src/venues/bsc_dex/exec/executor/live_row.rs, правило — live_rule.rs; так же писалась запись
  tests/data/executor_v3_live_2026-11-17): по пулу —
  (а) case «a»: execute X от W «A → B» поверх блока H (eth_simulateV1), получено = возврат execute, quoter_out — квотер
      на H (eth_call до симуляции, то же состояние), кредит Swap менеджера, x_left — balanceOf(X) по A и B после вызова;
  (б) case «b»: в ОДНОЙ симуляции: (а) заново, квотер B → A на выход (а) (quoter_out_after_a — то же состояние, что у
      продажи), продажа B → A, x_left; quoter_out — квотер продажи на H (до покупки; правило его не сравнивает).
      У Balancer V3 квотер меняет состояние (ревью t233, блокер 1) — квотер после (а) и продажа — в двух симуляциях
      поверх H, обе начинаются с той же (а) (см. case_ab).
  Ключи x_left — «usdt» (токен A, вход (а)) и «coin» (B) — имена формата записи 17.11 (live_row.rs), не монета USDT.
  Подбор суммы — по квотеру на H до симуляции: наибольшая сумма из ряда 10^(d+3) … 10^(d−6) (d — знаков A), у которой
  выход > 0 и не меньше (1 − --max-impact-pct/100)·10·выход(сумма/10) (пул держит сумму без большого сдвига цены).
  Пулы выбираются ДО симуляции (по квотеру), исход симуляции на выбор не влияет.
Кто вызывает: tools/arbitrum_record/exec_v4_live.py.
От чего зависит: v4live_rpc.py, v4live_mgr.py, v4live_old.py.
Ошибки: сбой узла — исключение как есть; откат и ошибка симуляции — строка записи / пропуск с причиной.
"""

import v4live_mgr as mgr
import v4live_old as old
from v4live_rpc import ad, enc_execute, parse_call, sel, uint_of, word


def quote_call(p, a, w_addr, tin, tout, amt):
    """Вызов квотера и разбор — по виду пула. Ошибок нет."""
    return (mgr if p["fam"] in ("v4", "bv3") else old).quote_call(p, a, w_addr, tin, tout, amt)


def quote_at(node, tag, p, a, w_addr, tin, tout, amt):
    """Квотер на H (eth_call). Возвращает: (выход, None) или (None, ошибка)."""
    c, dec = quote_call(p, a, w_addr, tin, tout, amt)
    r, e = node.call(c["to"], c["input"], tag, frm=c.get("from"))
    if r is None or len(r) < 66:
        return None, e or r
    return dec(r), None


def size(node, tag, p, a, w_addr, tin, tout):
    """Сумма входа по квотеру (см. шапку). Возвращает: (сумма, выход, ряд проб) или (None, None, ряд). Ошибок нет."""
    r, _ = node.call(tin, "0x" + sel("decimals()"), tag)
    d = word(r, 0) if r and len(r) >= 66 else 18
    probes, prev = [], None
    for k in range(d + 3, max(d - 6, 0) - 1, -1):
        amt = 10**k
        q, e = quote_at(node, tag, p, a, w_addr, tin, tout, amt)
        probes.append({"amount": str(amt), "out": str(q) if q is not None else None, "err": e})
        if prev and q and prev[1] and prev[1] * 100 >= (100 - a.max_impact_pct) * 10 * q:
            return prev[0], prev[1], probes
        prev = (amt, q)
    return None, None, probes


def step(p, tin, tout):
    """Шаг исполнителя (вся доля 10000 bps). Ошибок нет."""
    data = old.step_data(p, tin) if p["fam"] not in ("v4", "bv3") else p["data"]
    return dict(kind=p["kind"], pool=p["step_pool"], tokenIn=tin, tokenOut=tout, shareBps=10000, data=data)


def _bal_calls(w_addr, x, tin, tout):
    return [{"from": w_addr, "to": t, "input": "0x" + sel("balanceOf(address)") + ad(x)} for t in (tin, tout)]


def _xleft(c1, c2):
    """Остатки на X после вызова. Ревью t233, п. 5: если balanceOf(X) не ответил, x_left НЕ пишется (null) — у live_row.rs
    это «остаток не записан»: пул (а) / (б) не доказан, без ложного «на исполнителе остались токены» (объект с null
    разбирался бы как ненулевой остаток); причина — поле x_left_error строки и счёт в README записи. Возвращает:
    (x_left или None, причина или None). Ошибок нет."""
    v1, v2 = uint_of(c1), uint_of(c2)
    if v1 is None or v2 is None:
        bad = [k for k, v, c in (("usdt", v1, c1), ("coin", v2, c2)) if v is None]
        return None, "balanceOf(X) не ответил: " + ", ".join(f"{k} ({c.get('error') or c.get('returnData')})"
                                                            for k, _, c in (("usdt", v1, c1), ("coin", v2, c2))
                                                            if k in bad)
    return {"usdt": str(v1), "coin": str(v2)}, None


def _credit(p, logs, tout):
    return mgr.credit(p, logs, tout) if p["fam"] in ("v4", "bv3") else None


def _row(case, p, tin, tout, amt, res, xlr, qo, extra):
    xl, xerr = xlr
    got = int(res["returnData"], 16) if res["status"] and len(res["returnData"]) >= 66 else None
    cr = _credit(p, res["logs"], tout)
    d = dict(case=case, kind=p["kind"], pool_id=p["pool_id"], manager=p["manager"], pool_manager=p["pool_manager"],
             hooks=p["hooks"], token_in=tin, token_out=tout, amount_in=str(amt),
             got=str(got) if got is not None else None, quoter_out=str(qo) if qo is not None else None, ur_out=None,
             gas=res["gasUsed"], status=res["status"], revert=res["revert"], x_left=xl, swap_credit=cr,
             got_eq_swap_credit=(abs(cr["out_side"]) == got) if (cr and got is not None) else None)
    for k in ("bv3_hook", "bv3_hook_kind", "fee", "liquidity"):
        if k in p:
            d[k] = p[k]
    if got is not None and qo:
        d["diff_vs_quoter_pct"] = round(abs(got - qo) * 100 / qo, 6)
    if xerr:
        d["x_left_error"] = xerr
    d.update(extra)
    call = dict(case=case, pool_id=p["pool_id"], status=res["status"], returnData=res["returnData"],
                gasUsed=res["gasUsed"], logs=res["logs"], revert=res["revert"], error=res["error"], x_left=xl)
    return d, call


def case_ab(node, tag, p, a, ctx, tin, tout, amt, qa):
    """(а) и (б) по пулу. ctx: w, x, deadline, overrides. Возвращает: (строки summary, вызовы calls, откаты крючков,
    ошибка симуляции или None). Ошибок-исключений нет, кроме сетевых."""
    w_addr, x, ov = ctx["w"], ctx["x"], ctx["overrides"]
    rows, calls = [], []
    buy = {"from": w_addr, "to": x, "input": enc_execute(tin, amt, tout, 1, ctx["deadline"], [step(p, tin, tout)])}
    cs, err = node.simulate(tag, ov, [buy] + _bal_calls(w_addr, x, tin, tout))
    if cs is None:
        return rows, calls, f"eth_simulateV1 (а): {err}"
    ra = parse_call(cs[0])
    d, c = _row("a", p, tin, tout, amt, ra, _xleft(cs[1], cs[2]), qa, {"coin": tout})
    rows.append(d)
    calls.append(c)
    if not ra["status"]:
        return rows, calls, None
    m = int(ra["returnData"], 16)
    qs, qse = quote_at(node, tag, p, a, w_addr, tout, tin, m)
    qcall, qdec = quote_call(p, a, w_addr, tout, tin, m)
    sell = {"from": w_addr, "to": x, "input": enc_execute(tout, m, tin, 1, ctx["deadline"], [step(p, tout, tin)])}
    bal = _bal_calls(w_addr, x, tin, tout)
    if p["fam"] == "bv3":
        # Ревью t233, блокер 1: квотер Balancer V3 состояние НЕ откатывает — Router.querySwapSingleTokenExactIn →
        # Vault.quote (balancer-v3-monorepo@2b9ce4b1 pkg/vault/contracts/Router.sol L667-L692; VaultExtension.sol
        # L850-L874: quote лишь пересылает вызов, откат — только у quoteAndRevert L876-L893; RouterHooks.sol L361-L371:
        # querySwapHook → _swapHook — настоящий swap хранилища с записью балансов пула). В общей симуляции продажа шла бы
        # по сдвинутому квотером пулу — поэтому квотер «после (а)» — своя симуляция [buy, квотер] поверх H, продажа —
        # своя [buy, bal, bal, sell, bal, bal]. V4Quoter / QuoterV2 / резолвер Fluid / роутер LB отвечают через откат
        # или view — у них одна симуляция, как в записи 17.11.
        cq, errq = node.simulate(tag, ov, [buy, qcall])
        if cq is None:
            return rows, calls, f"eth_simulateV1 (б, квотер): {errq}"
        qc = cq[1]
        cs2, err2 = node.simulate(tag, ov, [buy] + bal + [sell] + bal)
        if cs2 is None:
            return rows, calls, f"eth_simulateV1 (б): {err2}"
        ra2, rb, xb = parse_call(cs2[0]), parse_call(cs2[3]), _xleft(cs2[4], cs2[5])
    else:
        cs2, err2 = node.simulate(tag, ov, [buy] + bal + [qcall, sell] + bal)
        if cs2 is None:
            return rows, calls, f"eth_simulateV1 (б): {err2}"
        ra2, qc, rb, xb = parse_call(cs2[0]), cs2[3], parse_call(cs2[4]), _xleft(cs2[5], cs2[6])
    rd = qc.get("returnData", "0x")
    q_after = qdec(rd) if qc.get("status") == "0x1" and len(rd) >= 66 else None
    extra = {"coin": tout, "quoter_out_after_a": str(q_after) if q_after else None, "quoter_err": qse,
             "a_rerun_got": str(int(ra2["returnData"], 16)) if ra2["status"] else None,
             "a_rerun_same": ra2["returnData"] == ra["returnData"],
             "quoter_after_a_sim": "отдельная [buy, квотер]" if p["fam"] == "bv3" else "та же, перед продажей"}
    d2, c2 = _row("b", p, tout, tin, m, rb, xb, qs, extra)
    if q_after and rb["status"]:
        d2["diff_vs_quoter_after_a_pct"] = round(abs(int(rb["returnData"], 16) - q_after) * 100 / q_after, 6)
    rows.append(d2)
    calls.append(c2)
    return rows, calls, None
