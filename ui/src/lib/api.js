// Файл: ui/src/lib/api.js
// Что делает: запросы к REST бэкенда (src/api). Ошибка бэкенда превращается в Error с его текстом как есть:
//   JSON {"error": "..."} → текст error; простой текст (например отказ api/guard.rs) → этот текст.
//   У ошибки есть поле status — HTTP-код (0 — запрос не дошёл): по нему экраны отличают
//   «не применимо» (400), «площадка не подключена» (404), «нет данных» (503) — src/api/README.md.
// Кто вызывает: все экраны и хранилища ui/src/lib/*.svelte.js; download — pages/Uncapped.svelte (решение 215).
// От чего зависит: fetch браузера.
// Настройки: нет. Адрес — тот же сервер, что отдал страницу (или proxy vite в разработке).

/**
 * Что делает: отправляет запрос и разбирает ответ.
 * Принимает: метод, путь (/api/...), тело (объект → JSON) или undefined.
 * Возвращает: разобранный JSON ответа (или null для пустого ответа).
 * Ошибки: Error с текстом бэкенда и status; Error «Нет связи с сервером: …» (status 0), если запрос не дошёл.
 */
export async function request(method, path, body) {
  const init = { method, headers: {} };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(path, init);
  } catch (e) {
    throw Object.assign(new Error(`Нет связи с сервером: ${e.message}`), { status: 0 });
  }
  const text = await res.text();
  let data = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!res.ok) {
    const reason = data && typeof data === 'object' && 'error' in data ? data.error : data;
    throw Object.assign(new Error(reason || `Ошибка ${res.status} ${res.statusText}`), { status: res.status });
  }
  return data;
}

/** GET. Принимает: путь. Возвращает: JSON. Ошибки: как request. */
export const get = (path) => request('GET', path);
/** POST. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const post = (path, body) => request('POST', path, body);
/** PUT. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const put = (path, body) => request('PUT', path, body);
/** DELETE с телом. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const del = (path, body) => request('DELETE', path, body);

/** Кодирует часть пути (имя секрета, id площадки). Принимает: строку. Возвращает: строку. */
export const seg = (s) => encodeURIComponent(s);

/**
 * Что делает: скачивает файл ответа (решение 215: отчёт Excel «Упущено») — сохраняет под именем из Content-Disposition.
 * Принимает: путь (/api/...), запасное имя файла. Возвращает: ничего.
 * Ошибки: Error с текстом бэкенда и status (как request); «Нет связи с сервером: …» (status 0).
 */
export async function download(path, fallbackName) {
  let res;
  try {
    res = await fetch(path);
  } catch (e) {
    throw Object.assign(new Error(`Нет связи с сервером: ${e.message}`), { status: 0 });
  }
  if (!res.ok) {
    const text = await res.text();
    let reason = text;
    try {
      reason = JSON.parse(text).error ?? text;
    } catch {
      // Простой текст — как есть.
    }
    throw Object.assign(new Error(reason || `Ошибка ${res.status} ${res.statusText}`), { status: res.status });
  }
  const blob = await res.blob();
  const name = /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ?? fallbackName;
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
