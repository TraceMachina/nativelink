import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

// In use for aceternity components
// Merge aceternity tailwindcss to global tailwindcss
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
