import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/theme.css';
import { App } from './App.tsx';

const params = new URLSearchParams(window.location.search);
const isShowcase = import.meta.env.DEV && params.get('showcase') === 'tokens';
const rootElement = document.getElementById('root');
if (!rootElement) throw new Error('#root element not found');
const root = createRoot(rootElement);

if (isShowcase) {
  import('./showcase/TokenShowcase').then(({ TokenShowcase }) => {
    root.render(
      <StrictMode>
        <TokenShowcase />
      </StrictMode>,
    );
  });
} else {
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
