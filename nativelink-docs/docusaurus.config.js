// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).
// There are various equivalent ways to declare your Docusaurus config.
// See: https://docusaurus.io/docs/api/docusaurus-config

import {themes as prismThemes} from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'NativeLink',
  tagline: 'An extremely fast and efficient build cache and remote executor for Bazel RBE',
  favicon: 'img/favicon.svg',

  url: 'https://docs.nativelink.dev',
  baseUrl: '/',

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          routeBasePath: '/',
          sidebarPath: './sidebars.js',
          editUrl: ({versionDocsDirPath, docPath}) =>
          `https://github.com/TraceMachina/nativelink/edit/main/nativelink-docs/docs/${docPath}`,
        },
        blog: {
          showReadingTime: true,
        },
        theme: {
          customCss: './src/css/custom.css',
        },
        googleAnalytics: {
          trackingID: 'G-LCQD1ZQL3Q',
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: 'img/hero-dark.png',
      colorMode: {
        defaultMode: 'dark',
        disableSwitch: false,
        respectPrefersColorScheme: false,
      },
      navbar: {
        logo: {
          alt: 'My Site Logo',
          src: 'img/light.svg',
          srcDark: 'img/dark.svg',
        },
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'tutorialSidebar',
            position: 'left',
            label: 'Docs',
          },
          {
            href: 'https://nativelink.substack.com/',
            label: 'Blog',
            position: 'right'},
          {
            href: 'https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A',
            label: 'Community',
            position: 'right',
          },
          {
            href: 'https://github.com/TraceMachina/nativelink',
            label: 'GitHub',
            position: 'right',
          },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {
                label: 'Docs',
                to: '/',
              },
            ],
          },
          {
            title: 'Community',
            items: [
              {
                label: 'Slack',
                href: 'https://join.slack.com/t/nativelink/shared_invite/zt-281qk1ho0-krT7HfTUIYfQMdwflRuq7A',
              },
              {
                label: 'Twitter',
                href: 'https://twitter.com/tracemachina',
              },
            ],
          },
          {
            title: 'More',
            items: [
              {
                label: 'Blog',
                href: 'https://nativelink.substack.com/',
              },
              {
                label: 'Github',
                href: 'https://github.com/TraceMachina/nativelink',
              },
            ],
          },
        ],
        copyright: `Copyright © ${new Date().getFullYear()} Trace Machina, Inc.`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ['bash', 'powershell'],
      },
    }),
};

export default config;
