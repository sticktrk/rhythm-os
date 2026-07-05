import { lazy, Suspense } from 'react';
import { Navigate, Route, Routes } from 'react-router-dom';
import { Loader2 } from 'lucide-react';

import DashboardPage from './pages/dashboard/DashboardPage';
import HubConsoleLayout from './pages/hub/HubConsoleLayout';

const OverviewPage = lazy(() => import('./pages/hub/OverviewPage'));
const NodesPage = lazy(() => import('./pages/hub/NodesPage'));
const ProfilesPage = lazy(() => import('./pages/hub/ProfilesPage'));
const ScenesPage = lazy(() => import('./pages/hub/ScenesPage'));
const ModesPage = lazy(() => import('./pages/hub/ModesPage'));
const InputsPage = lazy(() => import('./pages/hub/InputsPage'));
const TopologyPage = lazy(() => import('./pages/hub/TopologyPage'));
const EnvironmentPage = lazy(() => import('./pages/hub/EnvironmentPage'));
const HistoryPage = lazy(() => import('./pages/hub/HistoryPage'));
const RemotePage = lazy(() => import('./pages/hub/RemotePage'));
const SecurityPage = lazy(() => import('./pages/hub/SecurityPage'));
const SystemPage = lazy(() => import('./pages/hub/SystemPage'));
const ConsolePage = lazy(() => import('./pages/hub/ConsolePage'));

function PageFallback() {
  return (
    <div className="consolePageLoading">
      <Loader2 className="spin" size={20} />
    </div>
  );
}

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/" element={<DashboardPage />} />
      <Route path="/hubs/:hubId" element={<HubConsoleLayout />}>
        <Route index element={<Navigate to="overview" replace />} />
        <Route
          path="overview"
          element={
            <Suspense fallback={<PageFallback />}>
              <OverviewPage />
            </Suspense>
          }
        />
        <Route
          path="nodes"
          element={
            <Suspense fallback={<PageFallback />}>
              <NodesPage />
            </Suspense>
          }
        />
        <Route
          path="profiles"
          element={
            <Suspense fallback={<PageFallback />}>
              <ProfilesPage />
            </Suspense>
          }
        />
        <Route
          path="scenes"
          element={
            <Suspense fallback={<PageFallback />}>
              <ScenesPage />
            </Suspense>
          }
        />
        <Route
          path="modes"
          element={
            <Suspense fallback={<PageFallback />}>
              <ModesPage />
            </Suspense>
          }
        />
        <Route
          path="inputs"
          element={
            <Suspense fallback={<PageFallback />}>
              <InputsPage />
            </Suspense>
          }
        />
        <Route
          path="topology"
          element={
            <Suspense fallback={<PageFallback />}>
              <TopologyPage />
            </Suspense>
          }
        />
        <Route
          path="environment"
          element={
            <Suspense fallback={<PageFallback />}>
              <EnvironmentPage />
            </Suspense>
          }
        />
        <Route
          path="history"
          element={
            <Suspense fallback={<PageFallback />}>
              <HistoryPage />
            </Suspense>
          }
        />
        <Route
          path="remote"
          element={
            <Suspense fallback={<PageFallback />}>
              <RemotePage />
            </Suspense>
          }
        />
        <Route
          path="security"
          element={
            <Suspense fallback={<PageFallback />}>
              <SecurityPage />
            </Suspense>
          }
        />
        <Route
          path="system"
          element={
            <Suspense fallback={<PageFallback />}>
              <SystemPage />
            </Suspense>
          }
        />
        <Route
          path="console"
          element={
            <Suspense fallback={<PageFallback />}>
              <ConsolePage />
            </Suspense>
          }
        />
        <Route path="*" element={<Navigate to="overview" replace />} />
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
