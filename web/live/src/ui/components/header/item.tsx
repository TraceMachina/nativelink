import { Accessor, Component, createEffect, createSignal, on } from "solid-js";
import "./item.css";

interface NativelinkItemProps {
  connected: boolean;
  address?: string;
  lastUpdated: Accessor<number>;
}

/**
 * NativelinkItem component displays connection status in the header
 * It shows the server address and a visual indicator of connectivity
 */
export const NativelinkItem: Component<NativelinkItemProps> = (props) => {
  let iconRef!: HTMLSpanElement;
  const statusText = () => (props.connected ? "Connected" : "Disconnected");

  createEffect(
    on(
      props.lastUpdated,
      () => {
        if (!props.connected || !iconRef) return;
        iconRef.animate(
          [{ opacity: 0.6 }, { opacity: 1.0 }, { opacity: 0.6 }],
          {
            duration: 250,
            easing: "ease-in-out",
          },
        );
      },
      { defer: true },
    ),
  );

  return (
    <div
      class="nativelink-item"
      classList={{ connected: props.connected }}
      title={`Status: ${statusText()}`}
    >
      <span class="brand">Nativelink</span>

      <div class="connection-status">
        <span class="address" title={props.address}>
          {props.address}
        </span>

        <span
          class="status-indicator material-symbols-rounded"
          ref={iconRef}
          aria-label={statusText()}
        >
          sensors
        </span>
      </div>
    </div>
  );
};

/**
 * Divider component renders a visual divider line
 * It is used to separate items in the header
 */
export const Divider: Component = () => {
  return <div class="divider" aria-hidden="true" />;
};
