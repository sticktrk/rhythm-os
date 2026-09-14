import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import LocalApp from './local/LocalApp';
import './styles.css';
import './styles/console.css';
import './styles/controls.css';
import './styles/pages-phase6.css';
import './local/local.css';

createRoot(document.getElementById('root')!).render(<StrictMode><LocalApp /></StrictMode>);
