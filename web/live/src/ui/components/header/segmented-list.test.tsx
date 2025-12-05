import { render, fireEvent, cleanup } from "@solidjs/testing-library";

import * as SegmentedList from "./segmented-list";
import { expect, test } from "vitest";
import { createSignal } from "solid-js";

test("simple segmented list", () => {
  const [activeId, setActiveId] = createSignal<string>("");
  const { getByText } = render(() => (
    <SegmentedList.List activeId={activeId} setActiveId={setActiveId}>
      <SegmentedList.Item id="1" label="Item 1" disabled={false} />
      <SegmentedList.Item id="2" label="Item 2" disabled={false} />
      <SegmentedList.Item
        id="3"
        label="Item 3"
        disabled={false}
        extra={"123"}
      />
    </SegmentedList.List>
  ));

  const item1 = getByText("Item 1");
  const item2 = getByText("Item 2");
  expect(item1).toBeInTheDocument();
  expect(item2).toBeInTheDocument();
  expect(item1).not.toHaveClass("active");
  expect(item2).not.toHaveClass("active");
  fireEvent.click(item1);
  expect(item1).toHaveClass("active");
  expect(item2).not.toHaveClass("active");
  fireEvent.click(item2);
  expect(item1).not.toHaveClass("active");
  expect(item2).toHaveClass("active");
});
