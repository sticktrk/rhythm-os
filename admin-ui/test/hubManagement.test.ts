import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  createMemoryRouter,
  Link,
  Outlet,
  RouterProvider
} from 'react-router-dom';

import {
  canonicalDevicesForHub,
  configuredHubsFromState,
  HUB_LIST_BACK_TARGET,
  hueAuthorityBridgeForHub,
  hueAuthorityUpdateBody
} from '../src/pages/hub/hubManagement.ts';

test('address-scoped hub back link returns to the configured-hub list', async () => {
  const router = createMemoryRouter(
    [
      {
        path: '/hubs/:hubId',
        element: React.createElement(Outlet),
        children: [
          {
            path: 'integrations',
            element: React.createElement('h2', null, 'Configured hubs')
          },
          {
            path: 'integrations/:integrationType/:integrationAddress',
            element: React.createElement(
              Link,
              { to: HUB_LIST_BACK_TARGET, relative: 'path' },
              'Back to hubs'
            )
          }
        ]
      }
    ],
    {
      initialEntries: ['/hubs/demo/integrations/hue/192.0.2.25']
    }
  );

  const detailMarkup = renderToStaticMarkup(
    React.createElement(RouterProvider, { router })
  );
  const href = detailMarkup.match(/href="([^"]+)"/)?.[1];
  assert.equal(href, '/hubs/demo/integrations');

  await router.navigate(href);

  const listMarkup = renderToStaticMarkup(
    React.createElement(RouterProvider, { router })
  );
  assert.equal(router.state.location.pathname, '/hubs/demo/integrations');
  assert.match(listMarkup, /Configured hubs/);
});

const state = {
  hubs: [
    { type: 'none', connected: false },
    { type: 'matter', address: 'local', connected: true },
    { type: 'hue', address: '192.0.2.26', connected: true },
    { type: 'hue', address: '192.0.2.25', connected: false },
    { type: 'local_ble', address: 'default', connected: true }
  ]
};

const devices = [
  {
    id: 'hue-one',
    endpoints: [
      { hub_key: { hub_type: 'hue', address: '192.0.2.25' } }
    ]
  },
  {
    id: 'hue-two',
    endpoints: [
      { hub_key: { hub_type: 'hue', address: '192.0.2.26' } }
    ]
  },
  {
    id: 'ble-button',
    endpoints: [
      { hub_key: { hub_type: 'local_ble', address: 'default' } }
    ]
  }
];

test('lists every configured hub including BLE and Matter', () => {
  const hubs = configuredHubsFromState(state);
  assert.deepEqual(
    hubs.map((hub) => `${hub.type}@${hub.address}`),
    [
      'hue@192.0.2.25',
      'hue@192.0.2.26',
      'local_ble@default',
      'matter@local'
    ]
  );
});

test('dedicated hub pages use exact type and address membership', () => {
  assert.deepEqual(
    canonicalDevicesForHub(devices, {
      type: 'hue',
      address: '192.0.2.25'
    }).map((device) => device.id),
    ['hue-one']
  );
  assert.deepEqual(
    canonicalDevicesForHub({ devices }, {
      type: 'local_ble',
      address: 'default'
    }).map((device) => device.id),
    ['ble-button']
  );
});

test('matches Hue authority with or without the bridge port', () => {
  const bridge = hueAuthorityBridgeForHub(
    {
      bridges: [
        { address: '192.0.2.25:443', revision: 'rev-one', rooms: [] },
        { address: '192.0.2.26:443', revision: 'rev-two', rooms: [] }
      ]
    },
    '192.0.2.25'
  );
  assert.equal(bridge?.revision, 'rev-one');
});

test('Hue save payload carries explicit owners and topology opt-in', () => {
  assert.deepEqual(
    hueAuthorityUpdateBody({
      bridge: {
        address: '192.0.2.25:443',
        revision: 'rev-one',
        rooms: [
          { room_id: 'office', owner: 'hue' },
          { room_id: 'hall', owner: 'hue' }
        ]
      },
      owners: { office: 'rhythm', hall: 'rhythm' },
      correlationId: 'admin-review-1',
      topologySyncEnabled: true
    }),
    {
      address: '192.0.2.25:443',
      revision: 'rev-one',
      correlation_id: 'admin-review-1',
      topology_sync_enabled: true,
      rooms: [
        { room_id: 'office', owner: 'rhythm' },
        { room_id: 'hall', owner: 'rhythm' }
      ]
    }
  );
});
