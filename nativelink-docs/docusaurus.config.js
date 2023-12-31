// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).
// There are various equivalent ways to declare your Docusaurus config.
// See: https://docusaurus.io/docs/api/docusaurus-config

import {themes as prismThemes} from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'NativeLink',
  tagline: 'Dinosaurs are cool',
  favicon: 'img/favicon.svg',

  // Set the production url of your site here
  url: 'https://docs.nativelink.dev',
  // Set the /<baseUrl>/ pathname under which your site is served
  // For GitHub pages deployment, it is often '/<projectName>/'
  baseUrl: '/',

  // GitHub pages deployment config.
  // If you aren't using GitHub pages, you don't need these.
  organizationName: 'NativeLink', // Usually your GitHub org/user name.
  projectName: 'nativelink-docs', // Usually your repo name.

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
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
          // Please change this to your repo.
          // Remove this to remove the "edit this page" links.
        },
        blog: {
          showReadingTime: true,
          // Please change this to your repo.
          // Remove this to remove the "edit this page" links.
        },
        theme: {
          customCss: './src/css/custom.css',
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      // Replace with your project's social card
      image: 'img/hero-dark.png',
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
                to: '/quickstart',
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
                label: 'GitHub',
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
      },
    }),
};

export default config;
