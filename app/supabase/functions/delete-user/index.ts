import {
  errorMessage,
  jsonResponse,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

Deno.serve(async (req) => {
  return withAuthenticatedRequest(req, async ({ userId, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    console.log(`Deleting user: ${userId}`)

    try {
      // Delete user data from tables.
      // Order matters if there are no ON DELETE CASCADE constraints.

      const { error: prefError } = await adminClient
        .from('preferences')
        .delete()
        .eq('user_id', userId)
      if (prefError) {
        console.log(
          'Preferences delete error (may not exist):',
          prefError.message,
        )
      }

      const { error: roomsError } = await adminClient
        .from('rooms')
        .delete()
        .eq('user_id', userId)
      if (roomsError) {
        console.log('Rooms delete error (may not exist):', roomsError.message)
      }

      const { error: hubsError } = await adminClient
        .from('hubs')
        .delete()
        .eq('user_id', userId)
      if (hubsError) {
        console.log('Hubs delete error (may not exist):', hubsError.message)
      }

      const { error: homesError } = await adminClient
        .from('homes')
        .delete()
        .eq('user_id', userId)
      if (homesError) {
        console.log('Homes delete error (may not exist):', homesError.message)
      }

      const { error: deleteError } = await adminClient.auth.admin.deleteUser(
        userId,
      )
      if (deleteError) {
        console.error('Auth user delete error:', deleteError.message)
        return jsonResponse({ error: deleteError.message }, 500)
      }

      console.log(`Successfully deleted user: ${userId}`)
      return jsonResponse({ success: true })
    } catch (error) {
      console.error('Delete user error:', error)
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
})
