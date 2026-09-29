import type { Chrome } from './chrome';

/** What the controller answers to a `preview` or `submit` call. */
export interface Preview {
  /** What Enter does with the text. */
  kind: 'empty' | 'open' | 'search' | 'invalid';
  /** Text the omnibox shows for this kind. */
  label: string;
}

/** The calls and reports the address bar makes to the controller. */
export interface AddressHost {
  /** Classifies `text` without acting on it. */
  preview(text: string): Promise<Preview>;
  /** Classifies `text` and, unless it is invalid, opens it. */
  submit(text: string): Promise<Preview>;
  /** Reports that the user abandoned the address bar. */
  cancel(): void;
  /** Reports a click on the omnibox while the address bar is closed. */
  openAddress(): void;
}

/**
 * The address bar inside the omnibox. It refills the input each time the controller opens the bar,
 * shows what Enter does while the user types, and reports Enter, Esc and clicks to the controller.
 */
export class AddressBar {
  private readonly input: HTMLInputElement;
  private readonly preview: HTMLElement;
  private readonly host: AddressHost;
  private editing = false;
  private appliedEpoch = 0;
  private seq = 0;

  constructor(omnibox: HTMLElement, host: AddressHost) {
    this.input = omnibox.querySelector('.address-input') as HTMLInputElement;
    this.preview = omnibox.querySelector('.preview') as HTMLElement;
    this.host = host;
    this.input.addEventListener('input', (e) => {
      if (!(e as InputEvent).isComposing) {
        void this.refreshPreview();
      }
    });
    this.input.addEventListener('compositionend', () => {
      void this.refreshPreview();
    });
    this.input.addEventListener('keydown', (e) => this.onKeydown(e));
    omnibox.addEventListener('click', () => {
      if (!this.editing) {
        this.host.openAddress();
      }
    });
  }

  /**
   * Applies the chrome the controller pushed. A new `addressEpoch` in address mode refills the input
   * with the seed, selects it and focuses it; leaving address mode blurs the input.
   */
  apply(chrome: Chrome): void {
    this.editing = chrome.mode === 'address';
    if (!this.editing) {
      this.input.blur();
      return;
    }
    if (chrome.addressEpoch === this.appliedEpoch) {
      return;
    }
    this.appliedEpoch = chrome.addressEpoch;
    this.input.value = chrome.seed;
    this.input.focus();
    this.input.select();
    void this.refreshPreview();
  }

  private onKeydown(e: KeyboardEvent): void {
    if (e.isComposing || e.keyCode === 229) {
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      void this.submit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      this.host.cancel();
    }
  }

  private async refreshPreview(): Promise<void> {
    const seq = ++this.seq;
    try {
      const preview = await this.host.preview(this.input.value);
      if (seq === this.seq) {
        this.show(preview, false);
      }
    } catch (error) {
      console.error(error);
    }
  }

  private async submit(): Promise<void> {
    const seq = ++this.seq;
    try {
      const preview = await this.host.submit(this.input.value);
      if (seq === this.seq && preview.kind === 'invalid') {
        this.show(preview, true);
      }
    } catch (error) {
      console.error(error);
    }
  }

  private show(preview: Preview, rejected: boolean): void {
    this.preview.hidden = preview.kind === 'empty';
    this.preview.textContent = preview.kind === 'invalid' ? preview.label : `↵ ${preview.label}`;
    this.preview.classList.toggle('invalid', preview.kind === 'invalid');
    this.preview.classList.toggle('rejected', rejected);
  }
}
