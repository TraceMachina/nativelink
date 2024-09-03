import { component$ } from "@builder.io/qwik";
import logoDark from "src/assets/logo-dark.svg";

export const Logo = component$(() => {
  return (
    <div id="logo" class="flex justify-center items-center">
      <a href="https://www.nativelink.com">
        <img
          alt="NativeLink Logo"
          src={logoDark.src}
          width="376"
          height="100"
        />
      </a>
    </div>
  );
});
