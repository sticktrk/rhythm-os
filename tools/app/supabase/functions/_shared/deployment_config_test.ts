import { deepStrictEqual, throws } from 'node:assert/strict'
import { githubIssuesRepo, remoteAccessDomain } from './deployment_config.ts'

Deno.test('remote access requires an explicit valid domain and preserves its destination', () => {
  for (const input of [undefined, '', ' ', 'https://devices.example.com',
    'devices.example.com/path', '*.example.com', 'example', '-invalid.example']) {
    throws(() => remoteAccessDomain(input))
  }
  deepStrictEqual(remoteAccessDomain(' .Devices.Example.COM. '), 'devices.example.com')
  deepStrictEqual(remoteAccessDomain('devices.rhythm.lighting'), 'devices.rhythm.lighting')
})

Deno.test('GitHub routing has no upstream default or legacy repository rewrite', () => {
  deepStrictEqual(githubIssuesRepo(undefined), '')
  deepStrictEqual(githubIssuesRepo('  '), '')
  deepStrictEqual(githubIssuesRepo(' community/support '), 'community/support')
  deepStrictEqual(githubIssuesRepo('sticktrk/rhythm-app'), 'sticktrk/rhythm-app')
})
