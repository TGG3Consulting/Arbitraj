"""Собирает сетевую приёмку V5 Uniswap V4 без hooks на Base или Ethereum.

Запуск из корня репозитория: python scripts/record_v4_chain_executor.py base
Два первоначальных кандидата и закреплённый хеш блока хранятся в tests/data.
Коллектор использует штатные методы записи проекта, ключей и подписей нет.
"""
import json
import sys
from argparse import Namespace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools/arbitrum_record"))
sys.path.insert(0, str(ROOT / "tools/bsc_record"))
from exec_v5_bsc import read_runtime
from exec_v5_live import empty
import v4live_cases as cases
import v4live_slots as slots
from v4live_rpc import Node, ZERO, ad, enc_execute, keccak256, parse_call, sel, w


def dump(path, data):
    """Записывает JSON без изменения исходных ответов."""
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


def limit_cases(node, tag, pool, args, ctx, tin, tout, amount, rows):
    """Обе стороны с точным minOut и с minOut на единицу выше квотера, все из одного состояния."""
    qa = int(rows[0]["quoter_out"])
    qb = int(rows[1]["quoter_out_after_a"])
    address, executor = ctx["w"], ctx["x"]

    def trade(source, target, size, minimum):
        data = enc_execute(source, size, target, minimum, ctx["deadline"], [cases.step(pool,source,target)])
        return {"from":address, "to":executor, "input":data}

    def balance(token,who):
        return {"from":address,"to":token,"input":"0x"+sel("balanceOf(address)")+ad(who)}

    names = [
        ("input_before",balance(tin,address)), ("output_before",balance(tout,address)),
        ("buy_quote",cases.quote_call(pool,args,address,tin,tout,amount)[0]),
        ("buy_above",trade(tin,tout,amount,qa+1)), ("buy_exact",trade(tin,tout,amount,qa)),
        ("input_after_buy",balance(tin,address)), ("output_after_buy",balance(tout,address)),
        ("sell_quote",cases.quote_call(pool,args,address,tout,tin,qa)[0]),
        ("sell_above",trade(tout,tin,qa,qb+1)), ("sell_exact",trade(tout,tin,qa,qb)),
        ("input_after_sell",balance(tin,address)), ("output_after_sell",balance(tout,address)),
        ("executor_in",balance(tin,executor)), ("executor_out",balance(tout,executor))]
    res,err=node.simulate(tag,ctx["overrides"],[item for _,item in names])
    if err: raise ValueError(err)
    return dict(pool=pool["pool_id"], amount=str(amount), buy_quote=str(qa), sell_quote=str(qb),
                results={k:parse_call(r) for (k,_),r in zip(names,res,strict=True)})


def record(network):
    """Проверяет EIP-1898, ключи, ликвидность. Записывает обе стороны на двух размерах."""
    folder = ROOT / "tests/data" / ("executor_v5_" + network + "_2026-11-26")
    inputs = json.loads((folder / "candidates.json").read_text("utf8"))
    cfg = json.loads((ROOT / "config/first_run_dex.json").read_text("utf8"))
    evidence = json.loads((ROOT / f"tests/data/first_run_dex_2026-11-26/{network}.json").read_text("utf8"))
    profile = next(p["settings"] for p in cfg if p["network"] == network and p["settings"]["dex.kind"] == "uniswap_v4")
    node = Node(evidence["endpoint"])
    chain_id = int(node.ok("eth_chainId", []), 16)
    if chain_id != evidence["chain_id"]:
        raise ValueError("Узел другой сети")
    tag = evidence["selector"]
    header = node.ok("eth_getBlockByHash", [tag["blockHash"], False])
    stamp = int(header["timestamp"], 16)
    sha, runtime = read_runtime()
    usdt = inputs["usdt"]
    wrapped = inputs["wrapped_native"]
    who, x = ["0x" + keccak256(f"arb.v5.{network}.{name}".encode()).hex()[-40:] for name in ("wallet", "executor")]
    if not all(empty(node, tag, a) for a in (who, x)):
        raise ValueError("Один из адресов симуляции занят")
    observed = []
    pools = []
    token_slots = {}
    for candidate in inputs["pools"]:
        pid = candidate["id"]
        key = node.ok("eth_call", [{"to":profile["dex.position_manager"],"data":"0x"+sel("poolKeys(bytes25)")+pid[2:52].ljust(64,"0")},tag])
        if key.lower() != candidate["poolKey"].lower() or "0x"+keccak256(bytes.fromhex(key[2:])).hex() != pid:
            raise ValueError(f"PoolKey {pid} не подтверждён")
        t0,t1,_,_,hook = candidate["key"]
        if hook != ZERO or key[2:] != ad(t0)+ad(t1)+w(candidate["key"][2])+w(candidate["key"][3])+ad(hook):
            raise ValueError("Контроль ABI ключа и hook")
        if usdt not in (t0,t1) or ZERO in (t0,t1):
            raise ValueError("Нужны две валюты ERC-20, одна из них USDT своей сети")
        liquidity=node.ok("eth_call",[{"to":profile["dex.state_view"],"data":"0x"+sel("getLiquidity(bytes32)")+pid[2:]},tag])
        if int(liquidity,16)<=0:
            raise ValueError(f"У {pid} нет ликвидности")
        manager=profile["dex.pool_manager"].lower()
        pool=dict(kind=19,fam="v4",pool_id=pid,tokens=[t0,t1],hooks=hook,manager=manager,pool_manager=manager,
                  step_pool=manager,data=key)
        pools.append(pool)
        observed.append(dict(pool_id=pid,liquidity=liquidity,poolKey=key))
        for t in (t0,t1):
            if t not in token_slots:
                token_slots[t],err=slots.find(node,tag,t,who,x)
                if err: raise ValueError(f"{t}: {err}")
    dp=node.ok("eth_call",[{"to":usdt,"data":"0x"+sel("decimals()")},tag])
    amounts=[10**int(dp,16),100*10**int(dp,16)]
    overrides,err=slots.overrides(node,tag,token_slots,who,x,{usdt:max(amounts)})
    if err: raise ValueError(err)
    overrides[x]={"code":runtime,"stateDiff":{"0x"+w(0):"0x"+ad(who),"0x"+w(1):"0x"+ad(wrapped)}}
    ctx=dict(w=who,x=x,deadline=stamp+600,overrides=overrides)
    args=Namespace(v4_quoter=profile["dex.quoter"])
    rows,calls,limits=[],[],[]
    for pool in pools:
        tout=next(t for t in pool["tokens"] if t!=usdt)
        for amount in amounts:
            quote,err=cases.quote_at(node,tag,pool,args,who,usdt,tout,amount)
            if err or not quote: raise ValueError(f"{pool['pool_id']} {amount}: {err}")
            result,raw,err=cases.case_ab(node,tag,pool,args,ctx,usdt,tout,amount,quote)
            if err: raise ValueError(err)
            for r in result:
                r["chain_id"]=chain_id
                r["starting_usdt_raw"]=str(amount)
            limits.append(limit_cases(node,tag,pool,args,ctx,usdt,tout,amount,result))
            calls.extend(raw)
            rows.extend(result)
            print(network,pool["pool_id"],amount,[(r["case"],r["status"]) for r in result],flush=True)
    dump(folder/"min_out.json",limits)
    dump(folder/"summary.json",rows)
    dump(folder/"calls.json",calls)
    dump(folder/"slots.json",token_slots)
    dump(folder/"hooks_reject.json",[])
    dump(folder/"keys.json",observed)
    # Запросы eth_simulateV1 восстанавливаются коллектором. Полный ответ узла сохраняем для независимой проверки.
    dump(folder/"simulation_rpc.json",[r for r in node.log if r.get("method")=="eth_simulateV1"])
    (folder/"README.md").write_text(
        f"# ArbExecutor V5 · {network}\n\n- chainId: {chain_id}\n- sha256 симулированного bin-runtime: {sha}\n"
        f"- Блок {evidence['block']}, хеш `{tag['blockHash']}`\n- RPC: {node.url}\n- Владелец симуляции: `{who}`\n"
        f"- Исполнитель в симуляции: `{x}`\n- Источник: `scripts/record_v4_chain_executor.py`\n\n"
        "Два пула ERC-20 USDT/USDC у официального менеджера V4 с нулевым hook. PositionManager возвращает полный PoolKey, "
        "его keccak256 равен ID. Покупка 1 и 100 USDT, продажа полученного USDC. Квотер продажи вызывается в том же состоянии после покупки. "
        "В eth_simulateV1 изолированно заданы код и конструкторные слоты исполнителя, баланс и разрешения тестового адреса. "
        "Минимальный выход в основной записи равен 1. Дополнительная запись min_out.json сверяет точный выход и отказ при превышении на единицу, баланс владельца до и после каждого направления. Результат в минимальных единицах, кредит из Swap и остатки исполнителя — в summary.json. Симуляция не изменяет сеть.\n",
        encoding="utf8")


if __name__ == "__main__":
    if len(sys.argv)!=2 or sys.argv[1] not in {"base","ethereum"}:
        raise SystemExit("Укажите base или ethereum")
    record(sys.argv[1])
