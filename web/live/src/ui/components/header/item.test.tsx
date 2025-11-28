import { render, fireEvent, cleanup } from "@solidjs/testing-library";

import * as Item from "./item";
import { expect, test, describe, onTestFinished } from "vitest";
import { createSignal, mergeProps } from "solid-js";

describe("nativelink", () => {
  test("marked as offline", () => {
    const [lastUpdated, setLastUpdated] = createSignal(0);
    const props = {
      lastUpdated,
      connected: false,
    };
    const { getByText } = render(() => <Item.NativelinkItem {...props} />);
  });

  test("marked as online, animating", () => {
    const [lastUpdated, setLastUpdated] = createSignal(0);
    const props = {
      lastUpdated,
      connected: true,
      address: "localhost:0",
    };
    const { getByText } = render(() => <Item.NativelinkItem {...props} />);

    // it seems that this is automatically shutdown at cleanup
    setInterval(() => {
      setLastUpdated(Date.now());
    }, 1500);
  });
});
