import { Check } from "lucide-react";

export function Checkbox({
  checked,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  ariaLabel?: string;
}) {
  return <span className="checkbox-control"><input aria-label={ariaLabel} type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} /><Check aria-hidden="true" /></span>;
}
