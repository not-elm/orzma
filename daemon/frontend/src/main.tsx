import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/theme.css';
import { App } from './App.tsx';
import { loadFontConfig, preloadFonts } from './config/font';

async function bootstrap(): Promise<void> {
  await loadFontConfig();
  await preloadFonts();

  const params = new URLSearchParams(window.location.search);
  const isShowcase = import.meta.env.DEV && params.get('showcase') === 'tokens';
  const rootElement = document.getElementById('root');
  if (!rootElement) throw new Error('#root element not found');
  const root = createRoot(rootElement);

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

void bootstrap();
