// Файл: contracts/evmtest/lib.mjs
// Что делает: окружение EVM-теста ArbExecutor (решение 272): компиляция исходников (ArbExecutor — тем же solc-js 0.8.37 и
//   теми же настройками, что contracts/build.mjs, и сверка с contracts/out/; PancakePair/Factory/ERC20 — solc 0.5.16 из
//   pancake-swap-core@3b214306; WETH9 — solc 0.6.6 из v2-periphery@ed249913; решение 395 — PoolManager Uniswap V4 и
//   Vault / CLPoolManager / BinPoolManager PancakeSwap Infinity — solc 0.8.26 из vendor/v4-core, vendor/infinity-core), EVM ethereumjs (хардфорк Cancun — EIP-1153
//   transient), развёртывание и вызовы, разбор отката и событий.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: npm @ethereumjs/{evm, common, statemanager, util}, ethers (ABI), solc 0.8.37 / 0.5.16 / 0.6.6 / 0.8.26 —
//   версии в package.json / package-lock.json; contracts/*.sol, contracts/out/*, vendor/*, fixtures/*.
// Настройки: нет.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { Common, Chain, Hardfork } from '@ethereumjs/common';
import { EVM } from '@ethereumjs/evm';
import { DefaultStateManager } from '@ethereumjs/statemanager';
import { Account, Address, hexToBytes, bytesToHex } from '@ethereumjs/util';
import { Interface, AbiCoder } from 'ethers';

const require = createRequire(import.meta.url);
export const here = dirname(fileURLToPath(import.meta.url));
export const coder = AbiCoder.defaultAbiCoder();

/// Компиляция одного файла с импортами от корня исходников (solc нормализует «../x.sol» от имени файла).
/// Принимает: модуль solc, корень, путь файла от корня, имя контракта, настройки, замены префиксов импорта (remappings
/// foundry официальных репозиториев, решение 395: [['solmate/', 'lib/solmate/'], …]; по умолчанию нет).
/// Возвращает: { abi, bin, runtime }. Ошибки: исключение с текстом компилятора.
export function compile(solc, base, src, name, settings = {}, remap = []) {
  const file = join(base, src);
  const input = {
    language: 'Solidity',
    sources: { [src]: { content: readFileSync(file, 'utf8') } },
    settings: {
      ...settings,
      outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object', 'evm.deployedBytecode.object'] } },
    },
  };
  const find = (p) => {
    const r = remap.find(([a]) => p.startsWith(a));
    return { contents: readFileSync(join(base, r ? r[1] + p.slice(r[0].length) : p), 'utf8') };
  };
  const out = JSON.parse(solc.compile(JSON.stringify(input), { import: find }));
  const errs = (out.errors ?? []).filter((e) => e.severity === 'error');
  if (errs.length) throw new Error(errs.map((e) => e.formattedMessage).join('\n'));
  const c = out.contracts[src][name];
  return { abi: c.abi, bin: c.evm.bytecode.object, runtime: c.evm.deployedBytecode.object };
}

export const solc08 = require('solc');
export const solc05 = require('solc-0.5.16');
export const solc06 = require('solc-0.6.6');
// Решение 395 (t181): официальные Uniswap v4-core и PancakeSwap infinity-core — pragma 0.8.26 (foundry.toml: via_ir, cancun).
export const solc0826 = require('solc-0.8.26');

/// Артефакт из contracts/out/ (или out/v1/): код создания, код в сети, ABI.
export function artifact(dir) {
  const r = (f) => readFileSync(join(here, '..', dir, f), 'utf8').trim();
  return { bin: r('ArbExecutor.bin'), runtime: r('ArbExecutor.bin-runtime'), abi: JSON.parse(r('ArbExecutor.abi')) };
}

/// Цепь: EVM Cancun и счета с балансом. Поле ts (bigint) — время блока вызовов (проверка deadline); не задано —
/// блок по умолчанию ethereumjs (время 0).
export async function chain() {
  const common = new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Cancun });
  const sm = new DefaultStateManager();
  const evm = await EVM.create({ common, stateManager: sm });
  return { evm, sm, ts: undefined };
}

/// Блок с заданным временем — те же поля, что defaultBlock() @ethereumjs/evm 3.1.1 (dist/esm/evm.js), кроме timestamp.
function blockAt(ts) {
  return {
    header: {
      number: 0n,
      cliqueSigner: () => Address.zero(),
      coinbase: Address.zero(),
      timestamp: ts,
      difficulty: 0n,
      prevRandao: new Uint8Array(32),
      gasLimit: 0n,
      baseFeePerGas: undefined,
      getBlobGasPrice: () => undefined,
    },
  };
}

export const addr = (h) => Address.fromString(h);

export async function fund(c, a, wei) {
  await c.sm.putAccount(addr(a), Account.fromAccountData({ balance: wei }));
}

export async function balance(c, a) {
  return (await c.sm.getAccount(addr(a)))?.balance ?? 0n;
}

/// Баланс счёта без вызова (код и хранилище не трогаются) — как BNB, присланный в обход receive (selfdestruct).
export async function setBalance(c, a, wei) {
  const acc = await c.sm.getAccount(addr(a));
  acc.balance = wei;
  await c.sm.putAccount(addr(a), acc);
}

/// Вызов. Принимает: цепь, от кого, кому (null — создание), данные hex, value. Возвращает: { ok, ret, reason, logs,
/// created, gas }. Ошибок нет (откат — ok = false и текст причины Error(string)).
export async function call(c, from, to, data, value = 0n) {
  const r = await c.evm.runCall({
    caller: addr(from),
    origin: addr(from),
    to: to ? addr(to) : undefined,
    data: hexToBytes(data.startsWith('0x') ? data : '0x' + data),
    value,
    gasLimit: 30_000_000n,
    block: c.ts === undefined ? undefined : blockAt(c.ts),
  });
  const e = r.execResult;
  const ret = bytesToHex(e.returnValue);
  let reason = e.exceptionError ? e.exceptionError.error : null;
  if (reason && ret.startsWith('0x08c379a0')) reason = coder.decode(['string'], '0x' + ret.slice(10))[0];
  const logs = (e.logs ?? []).map(([a, topics, d]) => ({
    address: bytesToHex(a),
    topics: topics.map(bytesToHex),
    data: bytesToHex(d),
  }));
  return { ok: !e.exceptionError, ret, reason, logs, created: r.createdAddress?.toString(), gas: e.executionGasUsed };
}

/// Развёртывание: код создания + аргументы конструктора. Ошибки: исключение с причиной отката.
export async function deploy(c, from, art, types = [], args = [], value = 0n) {
  const r = await call(c, from, null, '0x' + art.bin + coder.encode(types, args).slice(2), value);
  if (!r.ok) throw new Error(`развёртывание откатилось: ${r.reason}`);
  return r.created;
}

/// Вызов по ABI; ошибка — исключение, если ожидался успех. Возвращает итог call.
export async function send(c, from, to, abi, fn, args, value = 0n) {
  return call(c, from, to, new Interface(abi).encodeFunctionData(fn, args), value);
}

export async function must(c, from, to, abi, fn, args, value = 0n) {
  const r = await send(c, from, to, abi, fn, args, value);
  if (!r.ok) throw new Error(`${fn}: откат ${r.reason}`);
  return r;
}

export async function view(c, to, abi, fn, args = []) {
  const i = new Interface(abi);
  const r = await call(c, '0x' + '11'.repeat(20), to, i.encodeFunctionData(fn, args));
  return i.decodeFunctionResult(fn, r.ret)[0];
}

/// Проверка с текстом. Ошибка — исключение.
export function check(cond, text) {
  if (!cond) throw new Error(`НЕ ПРОШЛО: ${text}`);
  console.log(`ok — ${text}`);
}
