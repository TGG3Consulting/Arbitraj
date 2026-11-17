// Файл: contracts/evmtest/singleton.mjs
// Что делает: решение 390 (t181) — шаги UNI_V4 (19), INF_CL (20), INF_BIN (21) исполнителя ArbExecutor v3 на EVM
//   ethereumjs (Cancun) с ОФИЦИАЛЬНЫМ кодом менеджеров, собранным здесь же solc 0.8.26 (настройки foundry.toml
//   репозиториев: via_ir, cancun, optimizer_runs 44444444 / 25666, bytecode_hash none):
//   Uniswap/v4-core@bea7f6d1 src/PoolManager.sol (+ src/test/FeeTakingHook.sol — официальный крючок с дельтой) и
//   pancakeswap/infinity-core@3c5ef1be src/Vault.sol, src/pool-cl/CLPoolManager.sol, src/pool-bin/BinPoolManager.sol
//   (vendor/v4-core, vendor/infinity-core — файлы без изменений, хеши — vendor/SHA256SUMS). Ликвидность кладёт фикстура
//   fixtures/SingletonLp.sol; крючок повторного входа — fixtures/ReenterHook.sol.
//   G1) UNI_V4: USDT → COIN и обратно одним шагом — владелец получил ровно выход события Swap менеджера, списано ровно
//       столько, сколько менеджер взял; minOut + 1 — откат «minOut»; на исполнителе и у менеджера в дельтах — пусто;
//   G2) UNI_V4 с официальным FeeTakingHook (afterSwap берёт 1,23 % выхода дельтой крючка): владелец получил выход
//       события − комиссия крючка (дельта исполнителя у менеджера, а не выход пула);
//   G3) INF_CL и G4) INF_BIN: то же через Vault.lock / lockAcquired, обе стороны;
//   G5) сплит одной транзакцией, как у NFP: доли V2 (PancakePair) 40 % + UNI_V4 30 % + INF_CL остаток, затем цикл
//       USDT →(UNI_V4)→ COIN →(INF_BIN)→ USDT (одна доля, вход маршрута = выход);
//   G6) защита: посторонний зовёт unlockCallback / lockAcquired — «pool»; крючок пула пытается повторно войти
//       (unlockCallback / lockAcquired с ДАННЫМИ ШАГА, uniswapV3SwapCallback) — все отказаны, обмен прошёл; токены шага не
//       валюты ключа — «key»; нативная валюта (address 0) — «native»; вид UNI_V4 по адресу Vault — откат; исполнитель
//       v2 (contracts/out/v2/) и v1 — «kind».
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, vendor/v4-core, vendor/infinity-core, fixtures/SingletonLp.sol, fixtures/ReenterHook.sol.
// Настройки: нет.

import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, view, coder } = L;
const { E18, OWNER, LP, OTHER, ERC20, v1, v2, v2f, world, v2Step, exe, bal } = W;
const remap = [['solmate/', 'lib/solmate/'], ['@openzeppelin/', 'lib/openzeppelin-contracts/']];
const ir = (runs) => ({ optimizer: { enabled: true, runs }, viaIR: true, evmVersion: 'cancun', metadata: { bytecodeHash: 'none' } });
const v4dir = join(L.here, 'vendor', 'v4-core');
const infdir = join(L.here, 'vendor', 'infinity-core');
const PM = L.compile(L.solc0826, v4dir, 'src/PoolManager.sol', 'PoolManager', ir(44444444), remap);
const FEEHOOK = L.compile(L.solc0826, v4dir, 'src/test/FeeTakingHook.sol', 'FeeTakingHook', ir(44444444), remap);
const VAULT = L.compile(L.solc0826, infdir, 'src/Vault.sol', 'Vault', ir(25666), remap);
const CLPM = L.compile(L.solc0826, infdir, 'src/pool-cl/CLPoolManager.sol', 'CLPoolManager', ir(25666), remap);
const BINPM = L.compile(L.solc0826, infdir, 'src/pool-bin/BinPoolManager.sol', 'BinPoolManager', ir(25666), remap);
const LPC = L.compile(L.solc08, join(L.here, 'fixtures'), 'SingletonLp.sol', 'SingletonLp', W.opt);
const RH = L.compile(L.solc08, join(L.here, 'fixtures'), 'ReenterHook.sol', 'ReenterHook', W.opt);

const V4KEY = 'tuple(address,address,uint24,int24,address)';
const INFKEY = 'tuple(address,address,address,address,uint24,bytes32)';
const UNI_V4 = 19, INF_CL = 20, INF_BIN = 21;
const MIN_TICK = -887220n, MAX_TICK = 887220n; // кратны tickSpacing 60
const Q96 = 2n ** 96n;
const LIQ = 10n ** 22n;
const ACTIVE = 2n ** 23n; // бин цены 1 (BinPoolManager: id 2^23 — цена 1)

const sort = (a, b) => (BigInt(a) < BigInt(b) ? [a, b] : [b, a]);
const v4Key = (c0, c1, hooks = '0x' + '00'.repeat(20)) => [c0, c1, 3000, 60, hooks];
const infKey = (c0, c1, pm, bin) => [c0, c1, '0x' + '00'.repeat(20), pm, 3000, '0x' + ((bin ? 10n : 60n) << 16n).toString(16).padStart(64, '0')];
const uniStep = (pm, key, tin, tout, bps = 10000) => [UNI_V4, pm, tin, tout, bps, coder.encode([V4KEY], [key])];
const infStep = (kind, vault, key, tin, tout, bps = 10000) => [kind, vault, tin, tout, bps, coder.encode([INFKEY], [key])];

/// Окружение: мир world.mjs (исполнитель art) + PoolManager V4, Vault с CL/Bin менеджерами, пулы USDT/COIN без крючка
/// (V4, CL, Bin) с ликвидностью, разрешения владельца исполнителю. Возвращает всё нужное сценариям.
async function env(art = v2) {
  const w = await world(art);
  const { c, usdt, coin } = w;
  const [c0, c1] = sort(usdt, coin);
  const pm = await L.deploy(c, LP, PM, ['address'], [LP]);
  const vault = await L.deploy(c, LP, VAULT);
  const clpm = await L.deploy(c, LP, CLPM, ['address'], [vault]);
  const binpm = await L.deploy(c, LP, BINPM, ['address'], [vault]);
  await must(c, LP, vault, VAULT.abi, 'registerApp', [clpm]);
  await must(c, LP, vault, VAULT.abi, 'registerApp', [binpm]);
  const lp = await L.deploy(c, LP, LPC);
  for (const t of [usdt, coin]) await must(c, LP, t, ERC20.abi, 'transfer', [lp, 100_000n * E18]);
  const k4 = v4Key(c0, c1);
  await must(c, LP, pm, PM.abi, 'initialize', [k4, Q96]);
  await must(c, LP, lp, LPC.abi, 'addV4', [pm, k4, [MIN_TICK, MAX_TICK, LIQ, '0x' + '00'.repeat(32)]]);
  const kc = infKey(c0, c1, clpm, false);
  await must(c, LP, clpm, CLPM.abi, 'initialize', [kc, Q96]);
  await must(c, LP, lp, LPC.abi, 'addCl', [vault, kc, [MIN_TICK, MAX_TICK, LIQ, '0x' + '00'.repeat(32)]]);
  const kb = infKey(c0, c1, binpm, true);
  await must(c, LP, binpm, BINPM.abi, 'initialize', [kb, ACTIVE]);
  await must(c, LP, lp, LPC.abi, 'addBin', [vault, kb, binMint()]);
  for (const t of [usdt, coin]) await must(c, OWNER, t, ERC20.abi, 'approve', [w.ex, 10n ** 30n]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  return { ...w, c0, c1, pm, vault, clpm, binpm, lp, k4, kc, kb };
}

/// Раскладка ликвидности Bin (LiquidityConfigurations.sol: [0,24) id, [24,88) доля Y, [88,152) доля X; доли в 1e18):
/// бины ниже активного — только Y, выше — только X, активный — оба; по 10 000 каждого токена.
function binMint() {
  const part = E18 / 6n;
  const cfg = [];
  for (let d = -5n; d <= 5n; d++) {
    const x = d >= 0n ? part : 0n;
    const y = d <= 0n ? part : 0n;
    cfg.push('0x' + ((x << 88n) | (y << 24n) | (ACTIVE + d)).toString(16).padStart(64, '0'));
  }
  const amt = 10_000n * E18;
  return [cfg, '0x' + ((amt << 128n) | amt).toString(16).padStart(64, '0'), '0x' + '00'.repeat(32)];
}

/// Суммы обмена из события Swap менеджера (amount0, amount1 — дельта вызывающего: минус — заплатил пулу).
function swapAmounts(r, mgr, abi) {
  const i = new Interface(abi);
  const t = i.getEvent('Swap').topicHash;
  const l = r.logs.filter((x) => x.address.toLowerCase() === mgr.toLowerCase() && x.topics[0] === t);
  return l.map((x) => i.decodeEventLog('Swap', x.data, x.topics)).map((e) => [BigInt(e.amount0), BigInt(e.amount1)]);
}

/// Обмен одним шагом (у исполнителя не осталось дельт у менеджера — иначе unlock / lock откатил бы всё: PoolManager.sol
/// L112, Vault.sol L80) и сверка с событием менеджера. Возвращает: { r, got, paid }.
async function one(e, step, tin, tout, amt, mgr, abi) {
  const { c, ex } = e;
  const trial = await (async () => {
    await c.sm.checkpoint();
    const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, 0n, [step]));
    const b = await bal(c, tout, OWNER);
    await c.sm.revert();
    return { r, b };
  })();
  check(trial.r.ok, `пробный обмен прошёл (${trial.r.reason ?? 'ok'})`);
  const before = await bal(c, tout, OWNER);
  const exp = trial.b - before;
  const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, exp + 1n, [step]));
  check(!bad.ok && bad.reason === 'minOut', 'minOut + 1 — откат «minOut»');
  const inB = await bal(c, tin, OWNER);
  const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, exp, [step]));
  check(r.ok, `обмен прошёл (газ ${r.gas})`);
  const got = (await bal(c, tout, OWNER)) - before;
  const paid = inB - (await bal(c, tin, OWNER));
  const ev = swapAmounts(r, mgr, abi);
  check(ev.length === 1, 'одно событие Swap менеджера');
  const [a0, a1] = ev[0];
  const zf = BigInt(tin) === BigInt(e.c0);
  const [evIn, evOut] = zf ? [-a0, a1] : [-a1, a0];
  check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n, 'на исполнителе ничего не осталось');
  return { r, got, paid, evIn, evOut };
}

/// Сценарии G1–G6. Возвращает запись для тестов бота (tests/data/executor_v3_evm_2026-11-17/singleton_steps.json):
/// шаги G1, прошедшие в EVM, — ключ пула, данные шага (байты, принятые контрактом), токены, вход, получено. Ошибок нет:
/// провал — check бросает.
export async function singleton() {
  const record = { note: 'EVM ethereumjs Cancun, contracts/out/ArbExecutor.bin-runtime v3, официальные менеджеры — contracts/evmtest/singleton.mjs', cases: [] };
  // Оценка газа развёртывания (для заметки владельцу): исполнение конструктора с записью кода (ethereumjs
  // executionGasUsed) + внутренняя цена транзакции создания: 21000 + 32000 + calldata (16 газа за ненулевой байт, 4 — за
  // нулевой; EIP-2028) + 2 газа за слово кода создания (EIP-3860).
  for (const [name, art] of [['v3 (текущая сборка)', v2], ['v2 (out/v2)', v2f]]) {
    const c = await L.chain();
    await L.fund(c, OWNER, 10n ** 24n);
    const w = await L.deploy(c, LP, W.WETH);
    const data = '0x' + art.bin + coder.encode(['address'], [w]).slice(2);
    const r = await L.call(c, OWNER, null, data);
    const bytes = Buffer.from(data.slice(2), 'hex');
    const cd = [...bytes].reduce((g, x) => g + (x ? 16n : 4n), 0n);
    const total = 21000n + 32000n + cd + 2n * BigInt(Math.ceil(bytes.length / 32)) + r.gas;
    check(r.ok, `развёртывание ${name}: газ ≈ ${total} (исполнение ${r.gas}, код создания ${bytes.length} байт)`);
    record[`deploy_gas_${name.slice(0, 2)}`] = total.toString();
  }
  console.log(`официальные менеджеры: PoolManager ${PM.runtime.length / 2} байт, Vault ${VAULT.runtime.length / 2}, CLPoolManager ${CLPM.runtime.length / 2}, BinPoolManager ${BINPM.runtime.length / 2}`);
  // G1, G3, G4.
  {
    const e = await env();
    const { usdt, coin, pm, vault, clpm, binpm, k4, kc, kb } = e;
    const amt = 100n * E18;
    for (const [name, mk, mgr, abi, key] of [
      ['UNI_V4', (a, b) => uniStep(pm, k4, a, b), pm, PM.abi, k4],
      ['INF_CL', (a, b) => infStep(INF_CL, vault, kc, a, b), clpm, CLPM.abi, kc],
      ['INF_BIN', (a, b) => infStep(INF_BIN, vault, kb, a, b), binpm, BINPM.abi, kb],
    ]) {
      for (const [tin, tout, dir] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
        const step = mk(tin, tout);
        const o = await one(e, step, tin, tout, amt, mgr, abi);
        record.cases.push({ name, kind: step[0], pool: step[1], token_in: tin, token_out: tout, key: key.map(String),
          data: step[5], amount_in: amt.toString(), got: o.got.toString(), event_in: o.evIn.toString(), event_out: o.evOut.toString(), gas: o.r.gas.toString() });
        check(o.got === o.evOut && o.got > 0n, `${name} ${dir}: владелец получил ровно выход события Swap менеджера (${o.got})`);
        check(o.paid === amt && o.evIn === amt, `${name} ${dir}: списано ровно amountIn, менеджер взял ровно его`);
      }
    }
  }
  // G2: официальный FeeTakingHook (флаги afterSwap + returnsDelta + add/remove с дельтой: 0x547 — Hooks.sol L33-L47).
  {
    const e = await env();
    const { c, usdt, coin, pm, lp, c0, c1 } = e;
    const tmp = await L.deploy(c, LP, FEEHOOK, ['address'], [pm]);
    const hook = '0x' + '11'.repeat(18) + '05' + '47';
    await c.sm.putContractCode(L.addr(hook), await c.sm.getContractCode(L.addr(tmp)));
    const kh = v4Key(c0, c1, hook);
    await must(c, LP, pm, PM.abi, 'initialize', [kh, Q96]);
    await must(c, LP, lp, LPC.abi, 'addV4', [pm, kh, [MIN_TICK, MAX_TICK, LIQ, '0x' + '00'.repeat(32)]]);
    const amt = 100n * E18;
    const hb = await bal(c, coin, hook); // крючок уже взял комиссию с добавления ликвидности (afterAddLiquidity)
    const o = await one(e, uniStep(pm, kh, usdt, coin), usdt, coin, amt, pm, PM.abi);
    const fee = (o.evOut * 123n) / 10000n;
    check(o.got === o.evOut - fee && fee > 0n,
      `UNI_V4 с FeeTakingHook: владелец получил выход пула ${o.evOut} − комиссия крючка ${fee} = ${o.got} (дельта исполнителя у менеджера)`);
    check((await bal(c, coin, hook)) - hb === fee, 'UNI_V4 с FeeTakingHook: комиссию обмена забрал крючок');
  }
  // G5: сплит V2 40 % + UNI_V4 30 % + INF_CL остаток; цикл UNI_V4 → INF_BIN.
  {
    const e = await env();
    const { c, ex, usdt, coin, pairCU, pm, vault, clpm, k4, kc, kb } = e;
    const amt = 300n * E18;
    const steps = [v2Step(pairCU, usdt, coin, 4000), uniStep(pm, k4, usdt, coin, 5000), infStep(INF_CL, vault, kc, usdt, coin)];
    const before = await bal(c, coin, OWNER);
    const ub = await bal(c, usdt, OWNER);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, steps));
    check(r.ok, `сплит V2 + UNI_V4 + INF_CL одной транзакцией прошёл (газ ${r.gas})`);
    const v4 = swapAmounts(r, pm, PM.abi)[0];
    const cl = swapAmounts(r, clpm, CLPM.abi)[0];
    const zf = BigInt(usdt) === BigInt(e.c0);
    const inOf = ([a0, a1]) => (zf ? -a0 : -a1);
    const outOf = ([a0, a1]) => (zf ? a1 : a0);
    check(inOf(v4) === (amt * 6000n / 10000n) / 2n && inOf(cl) === amt - amt * 4000n / 10000n - inOf(v4),
      'сплит: доли по bps от остатка — UNI_V4 взял 30 %, INF_CL — весь остаток');
    check(ub - (await bal(c, usdt, OWNER)) === amt, 'сплит: списано ровно amountIn');
    const got = (await bal(c, coin, OWNER)) - before;
    check(got > outOf(v4) + outOf(cl) && (await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n,
      `сплит: владелец получил выход трёх долей (${got}), на исполнителе пусто`);
    const ub2 = await bal(c, usdt, OWNER);
    const loop = await L.call(c, OWNER, ex, exe(usdt, 50n * E18, usdt, 0n, [uniStep(pm, k4, usdt, coin), infStep(INF_BIN, e.vault, kb, coin, usdt)]));
    check(loop.ok, `цикл USDT →(UNI_V4)→ COIN →(INF_BIN)→ USDT одной транзакцией прошёл (газ ${loop.gas})`);
    const back = (await bal(c, usdt, OWNER)) - (ub2 - 50n * E18);
    check(back > 0n && (await bal(c, coin, ex)) === 0n && (await bal(c, usdt, ex)) === 0n,
      `цикл: вернулось ${back} USDT из 50e18 (комиссии двух пулов), на исполнителе пусто`);
    const loopMin = await L.call(c, OWNER, ex, exe(usdt, 50n * E18, usdt, 50n * E18, [uniStep(pm, k4, usdt, coin), infStep(INF_BIN, e.vault, kb, coin, usdt)]));
    check(!loopMin.ok && loopMin.reason === 'minOut', 'цикл без прибыли с minOut = вход — откат «minOut»');
  }
  // G6: защита.
  {
    const e = await env();
    const { c, ex, usdt, coin, pm, vault, lp, c0, c1, k4, kc } = e;
    for (const fn of ['unlockCallback', 'lockAcquired']) {
      const r = await L.call(c, OTHER, ex, new Interface(v2.abi).encodeFunctionData(fn, ['0x']));
      check(!r.ok && r.reason === 'pool', `посторонний зовёт ${fn} — откат «pool»`);
      const r2 = await L.call(c, OWNER, ex, new Interface(v2.abi).encodeFunctionData(fn, ['0x']));
      check(!r2.ok && r2.reason === 'pool', `владелец зовёт ${fn} вне шага — откат «pool»`);
    }
    // Крючок повторного входа (BEFORE_SWAP = 1 << 7): с данными ровно этого шага.
    const tmp = await L.deploy(c, LP, RH);
    const hook = '0x' + '22'.repeat(18) + '00' + '80';
    await c.sm.putContractCode(L.addr(hook), await c.sm.getContractCode(L.addr(tmp)));
    const kh = v4Key(c0, c1, hook);
    await must(c, LP, pm, PM.abi, 'initialize', [kh, Q96]);
    await must(c, LP, lp, LPC.abi, 'addV4', [pm, kh, [MIN_TICK, MAX_TICK, LIQ, '0x' + '00'.repeat(32)]]);
    const amt = 10n * E18;
    const step = uniStep(pm, kh, usdt, coin);
    const cb = coder.encode(['uint8', 'address', 'address', 'uint256', 'address', 'bytes'], [UNI_V4, usdt, coin, amt, OWNER, step[5]]);
    await must(c, LP, hook, RH.abi, 'setTarget', [ex, cb]);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step]));
    check(r.ok && BigInt(await view(c, hook, RH.abi, 'tries')) === 3n,
      'крючок пытался войти в исполнитель 3 раза (unlockCallback / lockAcquired с данными шага, uniswapV3SwapCallback) — все отказаны, обмен прошёл');
    const k = await L.call(c, OWNER, ex, exe(usdt, amt, e.w, 0n, [uniStep(pm, k4, usdt, e.w)]));
    check(!k.ok && k.reason === 'key', 'токены шага не валюты ключа — откат «key»');
    const nk = [`0x${'00'.repeat(20)}`, c1, 3000, 60, `0x${'00'.repeat(20)}`];
    const n = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 0n, [uniStep(pm, nk, usdt, coin)]));
    check(!n.ok && n.reason === 'native', 'нативная валюта в ключе (address 0) — откат «native»');
    const wrongMgr = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 0n, [[UNI_V4, vault, usdt, coin, 10000, coder.encode([V4KEY], [k4])]]));
    check(!wrongMgr.ok, `вид UNI_V4 по адресу Vault Infinity — откат (${wrongMgr.reason})`);
    const clByPm = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 0n, [infStep(INF_CL, pm, kc, usdt, coin)]));
    check(!clByPm.ok, `вид INF_CL по адресу PoolManager V4 — откат (${clByPm.reason})`);
  }
  for (const [name, art] of [['v2', v2f], ['v1', v1]]) {
    const e = await env(art);
    const r = await L.call(e.c, OWNER, e.ex, exe(e.usdt, E18, e.coin, 0n, [uniStep(e.pm, e.k4, e.usdt, e.coin)]));
    check(!r.ok && r.reason === 'kind', `исполнитель ${name}: шаг UNI_V4 (19) — откат «kind» (вида нет в ${name})`);
    const r2 = await L.call(e.c, OWNER, e.ex, exe(e.usdt, E18, e.coin, 0n, [infStep(INF_CL, e.vault, e.kc, e.usdt, e.coin)]));
    check(!r2.ok && r2.reason === 'kind', `исполнитель ${name}: шаг INF_CL (20) — откат «kind»`);
  }
  return record;
}
