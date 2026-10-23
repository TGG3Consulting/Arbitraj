// Файл: ui/src/main.js
// Что делает: точка входа интерфейса — подключает стили и монтирует App.svelte в #app.
// Кто вызывает: index.html.
// От чего зависит: svelte, App.svelte, app.css.
// Настройки: нет.

import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

export default mount(App, { target: document.getElementById('app') });
