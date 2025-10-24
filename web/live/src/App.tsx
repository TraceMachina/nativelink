import { Component, createSignal } from 'solid-js';
import { SimpleHeader } from './ui/components/header/header';
import { List, Item } from './ui/components/header/segmented-list';
import { LiveActivityMonitor } from './views/LiveActivityMonitor';
import { WorkerStatusDashboard } from './views/WorkerStatusDashboard';
import { ClientConnectionOverview } from './views/ClientConnectionOverview';
import { LogStreamViewer } from './views/LogStreamViewer';
import { BuildInfoViewer } from './views/BuildInfoViewer';
import './App.css';

type ViewType = 'activities' | 'workers' | 'clients' | 'logs' | 'builds';

export const App: Component = () => {
  const [activeView, setActiveView] = createSignal<ViewType>('activities');

  return (
    <div class="app">
      <SimpleHeader
        left={
          <div class="brand">
            <img
              src="https://nativelink-cdn.s3.us-east-1.amazonaws.com/nativelink_favicon.png"
              alt="NativeLink"
              class="brand-logo"
            />
            <h1>NativeLink Live</h1>
          </div>
        }
        contents={
          <List activeId={activeView} setActiveId={setActiveView}>
            <Item id="activities" label="Live Activities" disabled={false} />
            <Item id="workers" label="Workers" disabled={false} />
            <Item id="clients" label="Clients" disabled={false} />
            <Item id="builds" label="Builds" disabled={false} />
            <Item id="logs" label="Logs" disabled={false} />
          </List>
        }
        right={
          <div class="connection-status">
            <span class="status-indicator connected"></span>
            <span>Connected</span>
          </div>
        }
      />

      <main class="main-content">
        {activeView() === 'activities' && <LiveActivityMonitor />}
        {activeView() === 'workers' && <WorkerStatusDashboard />}
        {activeView() === 'clients' && <ClientConnectionOverview />}
        {activeView() === 'builds' && <BuildInfoViewer />}
        {activeView() === 'logs' && <LogStreamViewer />}
      </main>
    </div>
  );
};
