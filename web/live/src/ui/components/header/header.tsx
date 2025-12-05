// left, contents, right
/**
 * Headers.
 *
 * Header component has three sections:
 * - Left: branding. should not shrink.
 * - Contents: navigations.
 * - Right: hambuger menus or etc. should not shrink.
 */

import { Component, JSX } from "solid-js";
import "./header.css";

export const SimpleHeader: Component<{
  className?: string;
  left?: JSX.Element;
  contents?: JSX.Element;
  right?: JSX.Element;
}> = (props) => {
  return (
    <header class={`header ${props.className ?? ""}`}>
      <div class="left">{props.left}</div>
      <div class="contents">{props.contents}</div>
      <div class="right">{props.right}</div>
    </header>
  );
};
