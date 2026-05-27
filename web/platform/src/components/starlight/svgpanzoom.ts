import "@beoe/pan-zoom/css/PanZoomUi.css";
import { PanZoomUi } from "@beoe/pan-zoom";

// for BEOE diagrams
const beoeContainers = Array.from(
  document.querySelectorAll<HTMLElement>(".beoe"),
);
for (const container of beoeContainers) {
  const element = container.firstElementChild;
  if (!(element instanceof HTMLElement || element instanceof SVGSVGElement)) {
    continue;
  }
  new PanZoomUi({ element, container }).on();
}

// for content images
const svgImages = Array.from(
  document.querySelectorAll<HTMLImageElement>(
    ".sl-markdown-content > img[src$='.svg' i]," +
      ".sl-markdown-content > p > img[src$='.svg' i]," +
      // for development environment
      ".sl-markdown-content > img[src$='f=svg' i]," +
      ".sl-markdown-content > img[src$='f=svg' i]",
  ),
);

for (const img of svgImages) {
  let element: HTMLElement = img;
  if (element.parentElement?.tagName === "PICTURE") {
    element = element.parentElement;
  }
  const container = document.createElement("figure");
  container.classList.add("beoe", "not-content");
  element.replaceWith(container);
  container.append(element);
  new PanZoomUi({ element, container }).on();
}
