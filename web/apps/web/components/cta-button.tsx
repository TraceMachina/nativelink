import type { Cta } from "@/lib/cta";
import { Button, type ButtonProps } from "@nativelink/ui";
import type { ReactNode } from "react";

interface CtaButtonProps extends Pick<ButtonProps, "variant" | "size" | "className"> {
  cta: Cta;
  /** Optional trailing decoration after the label, such as an arrow. */
  children?: ReactNode;
}

/**
 * A `Button` rendered from one of the canonical CTAs in `lib/cta.ts`.
 *
 * Defaults to the large size, except for the link variant: a text link takes
 * no size, otherwise the size's padding would offset it from the buttons.
 */
export function CtaButton({ cta, variant, size, className, children }: CtaButtonProps) {
  const resolvedSize = size ?? (variant === "link" ? undefined : "lg");
  return (
    <Button asChild variant={variant} size={resolvedSize} className={className}>
      <a
        href={cta.href}
        target={cta.external ? "_blank" : undefined}
        rel={cta.external ? "noreferrer" : undefined}
      >
        {cta.label}
        {children}
      </a>
    </Button>
  );
}
