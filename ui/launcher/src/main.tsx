import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { LauncherApp } from './App';
import './styles.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <LauncherApp />
  </StrictMode>,
);
