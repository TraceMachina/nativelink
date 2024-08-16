import "@beoe/pan-zoom/css/PanZoomUi.css";
import { PanZoomUi } from "@beoe/pan-zoom";

// for BEOE diagrams
const beoeContainers = document.querySelectorAll(".beoe");
for (const container of beoeContainers) {
  const element = container.firstElementChild;
  if (!element) {
    continue;
  }
  // @ts-expect-error
  new PanZoomUi({ element, container }).on();
}

// for content images
const svgImages = document.querySelectorAll(
  ".sl-markdown-content > img[src$='.svg' i]," +
    ".sl-markdown-content > p > img[src$='.svg' i]," +
    // for development environment
    ".sl-markdown-content > img[src$='f=svg' i]," +
    ".sl-markdown-content > img[src$='f=svg' i]",
);

for (const img of svgImages) {
  let element = img;
  if (element.parentElement?.tagName === "PICTURE") {
    element = element.parentElement;
  }
  const container = document.createElement("figure");
  container.classList.add("beoe", "not-content");
  element.replaceWith(container);
  container.append(element);
  // @ts-expect-error
  new PanZoomUi({ element, container }).on();
}
