/**
 * Like https://m3.material.io/components/segmented-buttons/overview,
 * it is intended to use for switching views or filtering content.
 *
 * TODO(ilsubyeega): Make better work of this; should be available in plain web api/css.
 */

import {
  Accessor,
  Component,
  createContext,
  createEffect,
  createMemo,
  createSignal,
  JSX,
  Setter,
  Show,
  Suspense,
  useContext,
} from "solid-js";
import "./segmented-list.css";

export type SegmentedListContextType = {
  activeId: Accessor<string>;
  setActiveId: Setter<string>;
};
export const SegmentedListContext = createContext<SegmentedListContextType>();

/**
 * List component that provides context for its children items.
 * It creates a context with the active item id and a function to set the active item.
 *
 * @param props.active - The id of the currently active item.
 * @param props.setActive - Function to set the active item by id.
 *
 * @example
 * <SegmentedList.List {active} {setActive}>
 *  <SegmentedList.ItemUnwrapped id="1">Unwrapped</SegmentedList.ItemUnwrapped>
 *  <SegmentedList.Item id="1" label="Item 1" disabled={false} />
 *  <SegmentedList.Item id="2" label="Item 2" disabled={false} />
 *  <SegmentedList.Item id="3" label="Item 3" disabled={true} />
 * </SegmentedList.List>
 */
export const List: Component<
  SegmentedListContextType & {
    children: JSX.Element;
  }
> = (props) => {
  // The ref to the list container
  let listRef!: HTMLDivElement;

  // The position and width of the active line
  const [position, setPosition] = createSignal({
    left: "0px",
    width: "0px",
  });
  createEffect(() => {
    if (!listRef) return;
    // get active element
    const activeElement = listRef.querySelector(
      `[item-id='${props.activeId()}']`,
    );
    if (activeElement) {
      const rect = activeElement.getBoundingClientRect();
      const parentRect = listRef.getBoundingClientRect();
      setPosition({
        left: `${rect.left - parentRect.left}px`,
        width: `${rect.width}px`,
      });
    } else {
      setPosition({
        left: `0px`,
        width: `0px`,
      });
    }
  });

  return (
    <div role="tablist" class="segmented-list" ref={listRef}>
      <SegmentedListContext.Provider
        value={{
          activeId: props.activeId,
          setActiveId: props.setActiveId,
        }}
      >
        <Suspense>{props.children}</Suspense>
      </SegmentedListContext.Provider>
      <ActiveLine {...position()} />
    </div>
  );
};

/**
 * Unwrapped Item component that only provides id and children props.
 * See {@link Item} for a wrapped version that provides id, label and disabled props.
 */
export const ItemUnwrapped: Component<{
  id: string;
  children: JSX.Element;
}> = (props) => {
  const context = useContext(SegmentedListContext);
  if (!context) {
    throw new Error(
      "SegmentedList.Item must be used within a SegmentedList.List",
    );
  }

  return (
    <div role="tab" class="segmented-list-item-unwrapped" item-id={props.id}>
      {props.children}
    </div>
  );
};

/**
 * Wrapped Item component that provides id, label and disabled props.
 *
 * @param props.id - The unique identifier for the item.
 * @param props.label - The text label to display for the item.
 * @param props.disabled - If true, the item is disabled and cannot be selected.
 */
export const Item: Component<{
  id: string;
  label: string;
  extra?: string;
  disabled: boolean;
}> = (props) => {
  const context = useContext(SegmentedListContext);
  if (!context)
    throw new Error(
      "SegmentedList.Item must be used within a SegmentedList.List",
    );

  const isActive = createMemo(() => context.activeId() === props.id);

  return (
    <ItemUnwrapped id={props.id}>
      <button
        role="tab"
        class="segmented-list-item-wrapped"
        classList={{
          active: isActive(),
          disabled: props.disabled,
        }}
        onClick={() => {
          if (!props.disabled) {
            context.setActiveId(props.id);
          }
        }}
        aria-selected={isActive()}
        aria-disabled={props.disabled}
        tabindex={isActive() ? -1 : 0}
      >
        {props.label}
        <Show when={props.extra}>
          <span class="extra">{props.extra}</span>
        </Show>
      </button>
    </ItemUnwrapped>
  );
};

const ActiveLine: Component<{
  left: string;
  width: string;
}> = (props) => {
  return (
    <div
      class="active-line"
      style={{
        left: props.left,
        width: props.width,
      }}
    ></div>
  );
};
