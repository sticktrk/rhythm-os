import { Construction } from 'lucide-react';

export default function PagePlaceholder({
  title,
  description
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>{title}</h2>
          <p className="pageIntro">{description}</p>
        </div>
      </header>
      <div className="consolePlaceholder">
        <Construction size={26} />
        <span>This section is being built. Use the Console page meanwhile.</span>
      </div>
    </div>
  );
}
