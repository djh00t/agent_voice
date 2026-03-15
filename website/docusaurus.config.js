/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'agent_voice',
  tagline: 'Release documentation for the Rust SIP voice bridge',
  favicon: 'img/mark.svg',
  url: 'https://example.com',
  baseUrl: '/',
  organizationName: 'agent-voice',
  projectName: 'agent_voice',
  onBrokenLinks: 'throw',
  i18n: {
    defaultLocale: 'en',
    locales: ['en']
  },
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'throw'
    }
  },
  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: 'docs',
          sidebarPath: require.resolve('./sidebars.js')
        },
        blog: false,
        theme: {
          customCss: require.resolve('./src/css/custom.css')
        }
      }
    ]
  ],
  themeConfig: {
    navbar: {
      title: 'agent_voice',
      items: [
        {to: '/docs/overview', label: 'Docs', position: 'left'},
        {to: '/docs/release', label: 'Release', position: 'left'}
      ]
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            {label: 'Overview', to: '/docs/overview'},
            {label: 'Configuration', to: '/docs/configuration'},
            {label: 'Release', to: '/docs/release'}
          ]
        }
      ],
      copyright: `Copyright ${new Date().getFullYear()} agent_voice`
    }
  }
};

module.exports = config;
