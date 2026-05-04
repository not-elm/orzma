import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/theme.css';
import { App } from './App.tsx';

async function bootstrap() {
  const params = new URLSearchParams(window.location.search);
  const isShowcase = import.meta.env.DEV && params.get('showcase') === 'tokens';
  const root = createRoot(document.getElementById('root')!);

  if (isShowcase) {
    const { TokenShowcase } = await import('./showcase/TokenShowcase');
    root.render(
      <StrictMode>
        <TokenShowcase />
      </StrictMode>,
    );
  } else {
    root.render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  }
}

bootstrap();
