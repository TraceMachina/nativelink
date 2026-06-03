"use client";

import * as motion from "motion/react-client";
import type * as React from "react";

interface RevealProps {
  delay?: number;
  y?: number;
  className?: string;
  children: React.ReactNode;
}

export function Reveal({ delay = 0, y = 12, className, children }: RevealProps) {
  return (
    <motion.div
      initial={{ opacity: 0, y }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, margin: "-80px" }}
      transition={{ duration: 0.6, delay, ease: [0.16, 1, 0.3, 1] }}
      className={className}
    >
      {children}
    </motion.div>
  );
}
