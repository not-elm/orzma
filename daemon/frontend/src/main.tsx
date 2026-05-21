import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles/theme.css';
import { App } from './App.tsx';
import { loadBrowserConfig } from './config/browser';
import { loadFontConfig, preloadFonts } from './config/font';
import { installPerfReport } from './terminal/perf/report';

installPerfReport();

async function bootstrap(): Promise<void> {
  await Promise.all([loadFontConfig(), loadBrowserConfig()]);
  await preloadFonts();

  const params = new URLSearchParams(window.location.search);
  const isShowcase = import.meta.env.DEV && params.get('showcase') === 'tokens';
  const replay = import.meta.env.DEV ? (params.get('replay') ?? undefined) : undefined;
  const recordPerf = import.meta.env.DEV ? params.get('record-perf') === '1' : false;
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
        <App replay={replay} recordPerf={recordPerf} />
      </StrictMode>,
    );
  }
}

void bootstrap();
