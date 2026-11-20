// Файл: contracts/evmtest/lb20.mjs
// Что делает: решение 458, T458-01 — шаг LB20 (24) исполнителя ArbExecutor v4 с ОФИЦИАЛЬНЫМ кодом Liquidity Book v2.0
//   (LFJ V2): исходники lfj-gg/joe-v2, тег v2.0.0 (07f10212e2de6abc97932240c27388c14868582c) — src/LBFactory.sol,
//   src/LBPair.sol, src/LBRouter.sol и их импорты, OpenZeppelin lib/openzeppelin-contracts@109778c1 (подмодуль тега)
//   (vendor/joe-v2, файлы без изменений, хеши — vendor/SHA256SUMS). Сборка — solc 0.8.10 (pragma `solidity 0.8.10`,
//   package-lock), настройки foundry.toml тега: optimizer = true, optimizer_runs = 800, evm_version не задан (по
//   умолчанию компилятора), remappings.txt: openzeppelin/ = lib/openzeppelin-contracts/contracts.
//   Развёртывание — как script/deploy-core.s.sol тега: LBFactory(владелец, FLASHLOAN_FEE 5e12) → LBPair(фабрика) →
//   LBRouter(фабрика, старая фабрика, обёртка) → setLBPairImplementation → addQuoteAsset; пресет шага бина 10 —
//   script/config/bips-config.sol. Пара USDT/COIN (COIN — монета котировки), активный бин 2^23 (цена 1), ликвидность —
//   перевод в пару + LBPair.mint (L501+) на 11 бинов. Котировка — официальный LBRouter.getSwapOut (L149-L190).
//   L1) обе стороны: владелец получил ровно getSwapOut = сумма amountOut событий Swap пары (по бинам), списано ровно
//       amountIn (прямой вход владелец → пара), выход — переводом пара → владелец, на исполнителе пусто; minOut + 1 — откат;
//   L1б) обмен через несколько бинов — получено = getSwapOut;
//   L2) две доли (50 % + остаток): вход с баланса исполнителя, оба обмена прошли, на исполнителе пусто;
//   L3) защита: «пара» шага — пара V2 (нет tokenX) — откат; чужая пара LB v2.0 (другие токены) — откат
//       LBPair__InsufficientAmounts, владелец при своих; шаг LB (18, интерфейс v2.1 getTokenX) по паре v2.0 — откат;
//       исполнитель v3 — «kind». Обратного вызова у шага нет (вход переводом, выход пара платит сама).
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, vendor/joe-v2.
// Настройки: нет.

import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, view } = L;
const { E18, OWNER, LP, ERC20, v2, world, exe, bal } = W;
const LB20 = 24;
const base = join(L.here, 'vendor', 'joe-v2');
const remap = [['openzeppelin/', 'lib/openzeppelin-contracts/contracts/']];
const opt = { optimizer: { enabled: true, runs: 800 } }; // foundry.toml тега v2.0.0
const FAC = L.compile(L.solc0810, base, 'src/LBFactory.sol', 'LBFactory', opt, remap);
const PAIR = L.compile(L.solc0810, base, 'src/LBPair.sol', 'LBPair', opt, remap);
const ROUTER = L.compile(L.solc0810, base, 'src/LBRouter.sol', 'LBRouter', opt, remap);
const P = new Interface(PAIR.abi);
const ACTIVE = 2n ** 23n;
// bips-config.sol, getPreset(10): binStep 10, baseFactor 10 000, filterPeriod 30, decayPeriod 600, reductionFactor 5 000,
// variableFeeControl 40 000, protocolShare 0, maxVolatilityAccumulated 350 000, sampleLifetime 120.
const PRESET10 = [10, 10_000, 30, 600, 5_000, 40_000, 0, 350_000, 120];
const step = (pair, tin, tout, bps = 10000, kind = LB20) => [kind, pair, tin, tout, bps, '0x'];

/// Пара LB v2.0 tokenX / tokenY (Y — монета котировки) с ликвидностью по 10 000 каждого токена в 11 бинах. Возвращает адрес.
async function pairOf(c, fac, x, y) {
  if (!(await view(c, fac, FAC.abi, 'isQuoteAsset', [y]))) await must(c, LP, fac, FAC.abi, 'addQuoteAsset', [y]);
  const r = await must(c, LP, fac, FAC.abi, 'createLBPair', [x, y, ACTIVE, 10]);
  const pair = new Interface(FAC.abi).decodeFunctionResult('createLBPair', r.ret)[0];
  const amt = 10_000n * E18;
  await must(c, LP, x, ERC20.abi, 'transfer', [pair, amt]);
  await must(c, LP, y, ERC20.abi, 'transfer', [pair, amt]);
  const ids = [], dx = [], dy = [];
  for (let d = -5n; d <= 5n; d++) {
    ids.push(ACTIVE + d);
    dx.push(d >= 0n ? E18 / 6n : 0n); // X — активный бин и выше
    dy.push(d <= 0n ? E18 / 6n : 0n); // Y — активный бин и ниже
  }
  await must(c, LP, pair, PAIR.abi, 'mint', [ids, dx, dy, LP]);
  return pair;
}

/// Мир: world.mjs (исполнитель art) + фабрика, реализация пары, роутер (deploy-core.s.sol), пары USDT/COIN и COIN/WETH.
async function env(art = v2) {
  const w = await world(art);
  const { c, usdt, coin, f, ex } = w;
  const fac = await L.deploy(c, LP, FAC, ['address', 'uint256'], [LP, 5n * 10n ** 12n]);
  const impl = await L.deploy(c, LP, PAIR, ['address'], [fac]);
  const router = await L.deploy(c, LP, ROUTER, ['address', 'address', 'address'], [fac, f, w.w]);
  await must(c, LP, fac, FAC.abi, 'setLBPairImplementation', [impl]);
  await must(c, LP, fac, FAC.abi, 'setPreset', PRESET10);
  const pair = await pairOf(c, fac, usdt, coin);
  await must(c, LP, w.w, W.WETH.abi, 'deposit', [], 20_000n * E18);
  const other = await pairOf(c, fac, w.w, coin);
  for (const t of [usdt, coin]) await must(c, OWNER, t, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  return { ...w, fac, router, pair, other };
}

/// Котировка LBRouter.getSwapOut(пара, вход, swapForY). Возвращает выход.
const quote = async (e, swapForY, amt) => BigInt((await L.call(e.c, OWNER, e.router,
  new Interface(ROUTER.abi).encodeFunctionData('getSwapOut', [e.pair, amt, swapForY]))).ret.slice(0, 66));

/// Сумма amountOut событий Swap пары (по бинам) в итоге вызова.
const swapOut = (r, pair) => r.logs.filter((l) => l.address.toLowerCase() === pair.toLowerCase() && l.topics[0] === P.getEvent('Swap').topicHash)
  .reduce((s, l) => s + BigInt(P.decodeEventLog('Swap', l.data, l.topics).amountOut), 0n);

export async function lb20() {
  const record = { note: 'EVM ethereumjs Cancun, contracts/out v4, официальный LBPair LFJ v2.0.0 (solc 0.8.10, runs 800) — contracts/evmtest/lb20.mjs', cases: [] };
  console.log(`официальный LFJ V2.0: LBPair ${PAIR.runtime.length / 2} байт, LBFactory ${FAC.runtime.length / 2}, LBRouter ${ROUTER.runtime.length / 2}`);
  {
    const e = await env();
    const { c, ex, pair, usdt, coin } = e;
    check((await view(c, pair, PAIR.abi, 'tokenX')).toLowerCase() === usdt.toLowerCase(), 'LB20: пара v2.0 создана фабрикой, tokenX = USDT, tokenY = COIN');
    const amt = 100n * E18;
    for (const [tin, tout, dir] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
      const q = await quote(e, tin === usdt, amt);
      const s = step(pair, tin, tout);
      const bad = await L.call(c, OWNER, ex, exe(tin, amt, tout, q + 1n, [s]));
      check(!bad.ok && bad.reason === 'minOut', `LB20 ${dir}: minOut = getSwapOut + 1 — откат «minOut»`);
      const ib = await bal(c, tin, OWNER);
      const ob = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, q, [s]));
      check(r.ok, `LB20 ${dir}: обмен прошёл (газ ${r.gas})`);
      const got = (await bal(c, tout, OWNER)) - ob;
      check(q > 0n && got === q && swapOut(r, pair) === q, `LB20 ${dir}: владелец получил ровно LBRouter.getSwapOut = сумма amountOut событий Swap пары (${got})`);
      check(ib - (await bal(c, tin, OWNER)) === amt, `LB20 ${dir}: списано ровно amountIn (прямой вход владелец → пара)`);
      check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n, `LB20 ${dir}: на исполнителе пусто`);
      record.cases.push({ kind: LB20, pair, token_in: tin, token_out: tout, amount_in: amt.toString(), got: got.toString(), gas: r.gas.toString() });
    }
    // Обмен через несколько бинов (в бине ~1 667 каждого токена): получено = getSwapOut, событий Swap > 1.
    const big = 3_000n * E18;
    const qb = await quote(e, true, big);
    const bb = await bal(c, coin, OWNER);
    const rb = await L.call(c, OWNER, ex, exe(usdt, big, coin, qb, [step(pair, usdt, coin)]));
    const nb = rb.logs.filter((l) => l.address.toLowerCase() === pair.toLowerCase() && l.topics[0] === P.getEvent('Swap').topicHash).length;
    check(rb.ok && nb > 1 && (await bal(c, coin, OWNER)) - bb === qb && swapOut(rb, pair) === qb,
      `LB20 через ${nb} бинов: владелец получил ровно getSwapOut (${qb})`);
    const cb = await bal(c, coin, OWNER);
    const ub = await bal(c, usdt, OWNER);
    const two = await L.call(c, OWNER, ex, exe(usdt, 20n * E18, coin, 1n, [step(pair, usdt, coin, 5000), step(pair, usdt, coin)]));
    const n = two.logs.filter((l) => l.address.toLowerCase() === pair.toLowerCase() && l.topics[0] === P.getEvent('Swap').topicHash).length;
    check(two.ok && n >= 2 && (await bal(c, coin, OWNER)) > cb && ub - (await bal(c, usdt, OWNER)) === 20n * E18
      && (await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n,
      'LB20 двумя долями (50 % + остаток): вход с баланса исполнителя, выход владельцу, на исполнителе пусто');
  }
  {
    const e = await env();
    const { c, ex, usdt, coin, other, pairCU, pair } = e;
    const v2p = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pairCU, usdt, coin)]));
    check(!v2p.ok, `LB20: «пара» шага — пара V2 (нет tokenX) — откат (${v2p.reason})`);
    const ub = await bal(c, usdt, OWNER);
    const fo = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(other, usdt, coin)]));
    check(!fo.ok && P.parseError(fo.ret)?.name === 'LBPair__InsufficientAmounts' && (await bal(c, usdt, OWNER)) === ub,
      'LB20: чужая пара LB v2.0 (WETH/COIN) — откат LBPair__InsufficientAmounts, владелец при своих');
    const v21 = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 0n, [step(pair, usdt, coin, 10000, 18)]));
    check(!v21.ok, `LB20: шаг LB (18, getTokenX v2.1) по паре v2.0 — откат (${v21.reason})`);
  }
  {
    const e = await env(W.v3f);
    const r = await L.call(e.c, OWNER, e.ex, exe(e.usdt, E18, e.coin, 0n, [step(e.pair, e.usdt, e.coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v3 (out/v3): шаг LB20 (24) — откат «kind»');
  }
  return record;
}
