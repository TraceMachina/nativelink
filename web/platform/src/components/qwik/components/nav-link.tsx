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
      <li class="relative flex flex-col justify-center items-center">
        <a
          {...props}
          class={[
            props.class || "",
            isActive ? activeClass : "",
            "px-3 py-2 min-h-[44px] flex items-center justify-center transition-colors duration-200 hover:text-[rgb(40,40,40)] text-sm",
          ]
            .filter(Boolean)
            .join(" ")}
        >
          <Slot />
        </a>
      </li>
    );
  },
);
