import { Slot, component$ } from "@builder.io/qwik";

type NavLinkProps = {
  pathName: string;
  activeClass?: string;
  href: string;
  class?: string;
};

export const NavLink = component$(
  ({ pathName, activeClass, ...props }: NavLinkProps) => {
    const toPathname = props.href ?? "";
    const locationPathname = pathName;

    // Check if the path is /resources or starts with /blog
    const isBlogOrResources =
      locationPathname.startsWith("/resources") ||
      locationPathname.startsWith("/blog");

    const startSlashPosition =
      toPathname !== "/" && toPathname.startsWith("/")
        ? toPathname.length - 1
        : toPathname.length;
    const endSlashPosition =
      toPathname !== "/" && toPathname.endsWith("/")
        ? toPathname.length - 1
        : toPathname.length;
    const isActive =
      locationPathname === toPathname ||
      (locationPathname.endsWith(toPathname) &&
        (locationPathname.charAt(endSlashPosition) === "/" ||
          locationPathname.charAt(startSlashPosition) === "/")) ||
      (toPathname === "/resources" && isBlogOrResources);

    return (
      <li class="relative flex flex-col justify-center items-center gap-10 w-32">
        <a
          {...props}
          class={`${props.class || ""} ${isActive ? activeClass : ""} group transition duration-300`}
        >
          <Slot />
          <span class="block max-w-0 group-hover:max-w-full transition-all duration-500 h-0.5 bg-white" />
        </a>
      </li>
    );
  },
);
