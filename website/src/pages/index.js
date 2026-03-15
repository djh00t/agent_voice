import React from 'react';
import Link from '@docusaurus/Link';
import Layout from '@theme/Layout';

export default function Home() {
  return (
    <Layout title="agent_voice" description="Release docs for agent_voice">
      <main className="hero">
        <div className="container">
          <h1>agent_voice</h1>
          <p>Release documentation for the Rust SIP voice bridge.</p>
          <p>
            <Link className="button button--primary" to="/docs/overview">
              Open documentation
            </Link>
          </p>
        </div>
      </main>
    </Layout>
  );
}
