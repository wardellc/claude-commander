import 'package:claude_commander_client/src/rust/api/mirrors.dart';
import 'package:claude_commander_client/theme/agent_glyphs.dart';
import 'package:claude_commander_client/util/agent_status.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/fixtures.dart';

void main() {
  test('unknown is visible and does not pretend to be idle', () {
    final d = sessionDescriptor(
      sessionInfo(status: SessionStatus.running),
      AgentState.unknown,
    );
    expect(d.label, 'unknown');
  });
  test('failure takes precedence over an unread completion marker', () {
    for (final state in [AgentState.failed, AgentState.rateLimited]) {
      final d = sessionDescriptor(
        sessionInfo(status: SessionStatus.running, unread: true),
        state,
      );
      expect(d.glyph, '!');
      expect(d.wantsAttention, isTrue);
      expect(d.label, isNot('unread'));
    }
  });
  test('startup, compaction and backoff remain in-flight work', () {
    for (final state in [
      AgentState.starting,
      AgentState.compacting,
      AgentState.retrying,
    ]) {
      expect(state.isBusy, isTrue);
      expect(
        sessionDescriptor(
          sessionInfo(status: SessionStatus.running),
          state,
        ).pulse,
        isTrue,
      );
    }
    expect(AgentState.waitingForApproval.needsAttention, isTrue);
    expect(AgentState.waitingForInput.needsAttention, isTrue);
    expect(AgentState.unknown.needsAttention, isFalse);
  });
}
