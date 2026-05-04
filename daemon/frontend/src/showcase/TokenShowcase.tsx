import { ColorSection } from './ColorSection';
import { RadiusSection } from './RadiusSection';
import { SpacingSection } from './SpacingSection';
import { TypographySection } from './TypographySection';

export function TokenShowcase() {
  return (
    <div className="min-h-screen bg-background text-foreground font-mono p-6">
      <h1 className="text-xl text-primary mb-6">Token Showcase</h1>
      <ColorSection />
      <TypographySection />
      <SpacingSection />
      <RadiusSection />
    </div>
  );
}
