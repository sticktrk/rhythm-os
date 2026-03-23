/// Universal user model that abstracts away provider-specific user types.
///
/// This model represents the authenticated user regardless of the
/// backend provider (Firebase, Supabase, etc.).
class AuthUser {
  /// Unique identifier for the user.
  final String id;

  /// User's email address (null for anonymous users).
  final String? email;

  /// User's display name.
  final String? displayName;

  /// Whether this is an anonymous/guest user.
  final bool isAnonymous;

  /// When the user account was created.
  final DateTime? createdAt;

  /// Provider-specific metadata (e.g., provider names, last sign-in).
  final Map<String, dynamic>? metadata;

  const AuthUser({
    required this.id,
    this.email,
    this.displayName,
    required this.isAnonymous,
    this.createdAt,
    this.metadata,
  });

  /// Create a copy with modified fields.
  AuthUser copyWith({
    String? id,
    String? email,
    String? displayName,
    bool? isAnonymous,
    DateTime? createdAt,
    Map<String, dynamic>? metadata,
  }) {
    return AuthUser(
      id: id ?? this.id,
      email: email ?? this.email,
      displayName: displayName ?? this.displayName,
      isAnonymous: isAnonymous ?? this.isAnonymous,
      createdAt: createdAt ?? this.createdAt,
      metadata: metadata ?? this.metadata,
    );
  }

  @override
  String toString() {
    return 'AuthUser(id: $id, email: $email, isAnonymous: $isAnonymous)';
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is AuthUser && runtimeType == other.runtimeType && id == other.id;

  @override
  int get hashCode => id.hashCode;
}
