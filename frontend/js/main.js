import { init } from './app/init.js';
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
else init();
