import { component$, useVisibleTask$ } from "@builder.io/qwik";
/*
  This script only runs on the client side and ensures that the client
  before issuing a request to the analytics tracking server. This code
  is experimental and should be removed if it causes and problems.
*/
declare global {
  interface Window {
    ldfdr?: {
      (...args: unknown[]): void;
      _q: unknown[];
    };
  }
}

const Visitors = component$(() => {
  useVisibleTask$(() => {

    const ss = "lAxoEaKMQGd7OYGd";

    window.ldfdr =
      window.ldfdr ||
      Object.assign(
        (...args: unknown[]) => {
          window.ldfdr?._q.push([...args]);
        },
        { _q: [] },
      );

    ((d, s) => {
      const fs = d.getElementsByTagName(s)[0];
      function ce(src: string) {
        const cs = document.createElement(s) as HTMLScriptElement;
        cs.src = src;
        cs.async = true;
        if (fs?.parentNode) {
          fs.parentNode.insertBefore(cs, fs);
        } else {
          d.head.appendChild(cs);
        }
      }
      ce(`https://sc.lfeeder.com/lftracker_v1_${ss}.js`);
    })(document, "script");
  });

  return null;
});

export default Visitors;
