import { component$ } from "@builder.io/qwik";

import { HiCheckSolid, HiXMarkSolid } from "@qwikest/icons/heroicons";

export const Checkmark = component$(() => {
  return (
    <div class="flex justify-center items-center">
      <HiCheckSolid class="text-2xl" style={{ color: "green" }} />
    </div>
  );
});

export const Xmark = component$(() => {
  return (
    <div class="flex justify-center items-center">
      <HiXMarkSolid class="text-2xl" style={{ color: "red" }} />
    </div>
  );
});
