const API_BASE = '/api';
const AUTH_BASE = '/auth';

const PROVIDER_NAMES = {
    openai: 'OpenAI',
    azure: 'Azure',
    anthropic: 'Anthropic',
    ollama: 'Ollama',
    vllm: 'vLLM',
    sglang: 'SGLang',
    minimax: 'MiniMax',
    custom: tp('custom')
};

const API_TYPE_NAMES = {
    openai: 'OpenAI API',
    anthropic: 'Anthropic API',
    ollama: 'Ollama API'
};

let currentUser = null;
let currentPage = 'login';
let systemBaseUrl = 'http://localhost:3000/v1';
let serverTimezoneOffsetMinutes = -new Date().getTimezoneOffset();
let serverTimezoneOffsetLabel = formatTimezoneOffset(serverTimezoneOffsetMinutes);
let auditUsersLoaded = false;
let auditCurrentPage = 1;
let auditTotalItems = 0;
const AUDIT_PAGE_SIZE = 20;
const UX_VARIANT_STORAGE_KEY = 'modelproxy-ui-variant';
const UX_METRICS_STORAGE_KEY = 'modelproxy-ui-metrics-v1';
const STALE_SESSION_MESSAGE = '__STALE_SESSION__';
const ERROR_MESSAGE_RULES = [
    { pattern: /invalid credentials/i, message: t('invalidCredentials') },
    { pattern: /user is disabled/i, message: t('accountDisabled') },
    { pattern: /invalid old password/i, message: t('currentPasswordIncorrect') },
    { pattern: /password verification failed/i, message: t('passwordVerifyFailed') },
    { pattern: /unauthorized|invalid token|token/i, message: t('invalidSession') },
    { pattern: /forbidden|permission denied/i, message: t('noPermission') },
    { pattern: /not found/i, message: t('contentNotFound') },
    { pattern: /conflict/i, message: t('dataConflict') },
    { pattern: /bad request/i, message: t('badRequest') },
    { pattern: /rate limit exceeded|too many requests/i, message: t('rateLimitExceeded') },
    { pattern: /payload too large/i, message: t('payloadTooLarge') },
    { pattern: /service unavailable/i, message: t('serviceUnavailable') },
    { pattern: /database error/i, message: t('systemBusy') },
    { pattern: /upstream error|bad gateway/i, message: t('upstreamError') },
    { pattern: /io error/i, message: t('ioError') },
    { pattern: /failed to fetch|network error|network request failed/i, message: t('networkErrorRetry') },
    { pattern: /timeout|timed out/i, message: t('requestTimeout') },
    { pattern: /copy failed/i, message: t('copyFailedManual') },
    { pattern: /save failed/i, message: t('saveFailedRetry') },
    { pattern: /export failed/i, message: t('exportFailedRetry') },
    { pattern: /request failed/i, message: t('requestFailedRetry') }
];
const ADMIN_PAGES = new Set([
    'users',
    'upstreams',
    'models',
    'conditional-aliases',
    'audit'
]);
const PAGE_TITLES = {
    dashboard: t('dashboard'),
    'api-keys': t('apiKeyManagement'),
    'my-models': t('myModels'),
    usage: t('usage'),
    users: t('userManagement'),
    upstreams: t('upstreamServices'),
    models: t('modelManagement'),
    'conditional-aliases': t('conditionalAliases'),
    audit: t('auditLogs'),
    'other-links': t('otherLinks'),
    settings: t('personalSettings')
};

let usageChart = null;
let echartsLoaderPromise = null;
let modalPreviousFocus = null;
const uxVariant = resolveUxVariant();

async function loadSystemSettings() {
    try {
        const response = await fetch(`${API_BASE}/settings/public`);
        if (response.ok) {
            const data = await response.json();
            systemBaseUrl = data.base_url;
            if (Number.isFinite(Number(data.server_timezone_offset_minutes))) {
                serverTimezoneOffsetMinutes = Number(data.server_timezone_offset_minutes);
                serverTimezoneOffsetLabel = data.server_timezone_offset || formatTimezoneOffset(serverTimezoneOffsetMinutes);
            }
        }
    } catch (e) {
        console.error('Failed to load system settings:', e);
    }
}

async function init() {
    applyTranslations();
    setupEventListeners();
    syncPageFromHash();
    await loadSystemSettings();
    document.body.dataset.uxVariant = uxVariant;
    recordUxMetric('session_start', { variant: uxVariant });
    const token = localStorage.getItem('token');
    if (token) {
        currentUser = JSON.parse(localStorage.getItem('user') || 'null');
        if (currentUser) {
            showApp();
            navigateTo(resolveAccessiblePage(getInitialPage(), currentUser));
            return;
        }
    } else {
        showLogin();
    }
    showLogin();
}

function setupEventListeners() {
    document.getElementById('login-form').addEventListener('submit', handleLogin);
    document.getElementById('register-form')?.addEventListener('submit', handleRegister);
    document.getElementById('toggle-register-btn')?.addEventListener('click', toggleRegisterPanel);
    document.getElementById('logout-btn').addEventListener('click', handleLogout);

    const langBtn = document.getElementById('lang-switch-btn');
    if (langBtn) {
        langBtn.addEventListener('click', () => {
            toggleLang();
            if (currentPage !== 'login') {
                loadPageData(currentPage);
            }
        });
    }
    const loginLangBtn = document.getElementById('login-lang-switch-btn');
    if (loginLangBtn) {
        loginLangBtn.addEventListener('click', () => {
            toggleLang();
        });
    }
    document.getElementById('sidebar-toggle')?.addEventListener('click', toggleSidebar);
    document.getElementById('sidebar-backdrop')?.addEventListener('click', () => setSidebarOpen(false));
    document.getElementById('page-refresh-btn')?.addEventListener('click', () => loadPageData(currentPage, { force: true }));

    document.querySelectorAll('.nav-link').forEach(link => {
        link.addEventListener('click', (e) => {
            e.preventDefault();
            const page = link.dataset.page;
            navigateTo(page);
        });
    });

    document.querySelector('.modal-close').addEventListener('click', hideModal);
    document.getElementById('modal')?.addEventListener('click', (event) => {
        if (event.target?.id === 'modal') {
            hideModal();
        }
    });
    document.addEventListener('keydown', handleGlobalKeydown);
    window.addEventListener('hashchange', syncPageFromHash);
    window.addEventListener('resize', handleWindowResize);

    document.getElementById('create-key-btn')?.addEventListener('click', showCreateKeyModal);
    document.getElementById('create-user-btn')?.addEventListener('click', showCreateUserModal);
    document.getElementById('refresh-pending-registrations-btn')?.addEventListener('click', loadPendingRegistrations);
    document.getElementById('create-upstream-btn')?.addEventListener('click', showCreateUpstreamModal);
    document.getElementById('create-other-link-btn')?.addEventListener('click', showCreateOtherLinkModal);
    document.getElementById('fetch-models-btn')?.addEventListener('click', loadModels);
    document.getElementById('add-manual-model-btn')?.addEventListener('click', addManualModel);
    document.getElementById('test-manual-model-btn')?.addEventListener('click', testManualModel);
    document.getElementById('create-conditional-alias-btn')?.addEventListener('click', () => editConditionalAlias());

    document.getElementById('usage-filter-btn')?.addEventListener('click', loadUsage);
    document.getElementById('audit-filter-btn')?.addEventListener('click', () => {
        auditCurrentPage = 1;
        loadAuditLogs();
    });
    document.getElementById('audit-prev-page-btn')?.addEventListener('click', () => {
        goToAuditPage(auditCurrentPage - 1);
    });
    document.getElementById('audit-next-page-btn')?.addEventListener('click', () => {
        goToAuditPage(auditCurrentPage + 1);
    });
    document.getElementById('audit-first-page-btn')?.addEventListener('click', () => {
        goToAuditPage(1);
    });
    document.getElementById('audit-last-page-btn')?.addEventListener('click', () => {
        goToAuditPage(getAuditTotalPages());
    });
    document.getElementById('audit-jump-page-btn')?.addEventListener('click', () => {
        const jumpValue = Number(document.getElementById('audit-jump-page-input')?.value || 1);
        goToAuditPage(jumpValue);
    });
    document.getElementById('audit-jump-page-input')?.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
            event.preventDefault();
            const jumpValue = Number(document.getElementById('audit-jump-page-input')?.value || 1);
            goToAuditPage(jumpValue);
        }
    });
    document.getElementById('audit-page-links')?.addEventListener('click', (event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) return;
        const button = target.closest('[data-page]');
        if (!button) return;
        const page = Number(button.getAttribute('data-page'));
        if (Number.isFinite(page)) {
            goToAuditPage(page);
        }
    });
    document.getElementById('audit-export-btn')?.addEventListener('click', exportAuditDataset);
    document.addEventListener('click', handleGlobalActions);
}

function normalizeErrorMessage(message, fallback = t('operationFailed')) {
    const text = String(message ?? '').trim().replace(/^error:\s*/i, '');
    if (!text) return fallback;
    if (text === STALE_SESSION_MESSAGE) return STALE_SESSION_MESSAGE;
    for (const rule of ERROR_MESSAGE_RULES) {
        if (rule.pattern.test(text)) {
            return rule.message;
        }
    }
    return text;
}

function handleGlobalActions(event) {
    const createAliasBtn = event.target.closest('#create-conditional-alias-btn');
    if (createAliasBtn) {
        event.preventDefault();
        openConditionalAliasEditor();
        return;
    }
    const editAliasBtn = event.target.closest('[data-action="edit-conditional-alias"]');
    if (editAliasBtn) {
        event.preventDefault();
        openConditionalAliasEditor(editAliasBtn.dataset.alias || '');
        return;
    }
    const visibilityBtn = event.target.closest('[data-action="edit-conditional-alias-visibility"]');
    if (visibilityBtn) {
        event.preventDefault();
        editConditionalAliasVisibility(visibilityBtn.dataset.alias || '');
        return;
    }
    const deleteBtn = event.target.closest('[data-action="delete-conditional-alias"]');
    if (deleteBtn) {
        event.preventDefault();
        deleteConditionalAlias(deleteBtn.dataset.alias || '');
    }
}

function showLogin() {
    currentPage = 'login';
    document.getElementById('sidebar').classList.add('hidden');
    document.getElementById('topbar')?.classList.add('hidden');
    setSidebarOpen(false);
    hideModal();
    document.getElementById('login-page').classList.remove('hidden');
    document.querySelectorAll('.page:not(#login-page)').forEach(p => p.classList.add('hidden'));
    document.querySelectorAll('.nav-link').forEach(link => {
        link.classList.remove('active');
        link.removeAttribute('aria-current');
    });
    document.body.classList.remove('app-ready');
    setRegisterPanelVisible(false);
    if (window.location.hash) {
        window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`);
    }
    document.title = t('loginPageTitle');
}

function showApp() {
    document.getElementById('sidebar').classList.remove('hidden');
    document.getElementById('topbar')?.classList.remove('hidden');
    document.getElementById('login-page').classList.add('hidden');
    
    document.getElementById('user-name').textContent = currentUser?.username || '-';
    document.getElementById('user-role').textContent = currentUser?.role || '-';
    
    if (currentUser?.role !== 'admin') {
        document.querySelectorAll('.admin-only').forEach(el => el.classList.add('hidden'));
    } else {
        document.querySelectorAll('.admin-only').forEach(el => el.classList.remove('hidden'));
    }

    document.body.classList.add('app-ready');
    prefetchNonCriticalAssets();
}

function getInitialPage() {
    const hashPage = normalizePageName(window.location.hash.replace(/^#/, ''));
    return hashPage || 'dashboard';
}

function isPageAccessible(page, user = currentUser) {
    if (!page || !user) {
        return false;
    }
    if (!Object.prototype.hasOwnProperty.call(PAGE_TITLES, page)) {
        return false;
    }
    return user.role === 'admin' || !ADMIN_PAGES.has(page);
}

function resolveAccessiblePage(page, user = currentUser) {
    const normalizedPage = normalizePageName(page);
    if (isPageAccessible(normalizedPage, user)) {
        return normalizedPage;
    }
    return 'dashboard';
}

function normalizePageName(page) {
    if (!page) return '';
    return page.replace(/[^a-z-]/g, '');
}

function syncPageFromHash() {
    const hashPage = getInitialPage();
    if (!hashPage || !currentUser) return;
    if (hashPage !== currentPage) {
        navigateTo(hashPage);
    }
}

function handleGlobalKeydown(event) {
    if (event.key === 'Escape') {
        if (!document.getElementById('modal')?.classList.contains('hidden')) {
            hideModal();
            return;
        }
        if (isMobileViewport()) {
            setSidebarOpen(false);
        }
    }
}

function handleWindowResize() {
    if (!isMobileViewport()) {
        setSidebarOpen(false);
    }
    if (usageChart) {
        usageChart.resize();
    }
}

function isMobileViewport() {
    return window.matchMedia('(max-width: 768px)').matches;
}

function toggleSidebar() {
    const sidebar = document.getElementById('sidebar');
    setSidebarOpen(!sidebar?.classList.contains('is-open'));
}

function setSidebarOpen(open) {
    const sidebar = document.getElementById('sidebar');
    const backdrop = document.getElementById('sidebar-backdrop');
    const toggle = document.getElementById('sidebar-toggle');
    if (!sidebar || !backdrop || !toggle) return;
    const shouldOpen = Boolean(open && isMobileViewport() && currentUser);
    sidebar.classList.toggle('is-open', shouldOpen);
    backdrop.classList.toggle('hidden', !shouldOpen);
    toggle.setAttribute('aria-expanded', shouldOpen ? 'true' : 'false');
}

function announcePage(page) {
    const title = PAGE_TITLES[page] || t('adminPanel');
    const topbarTitle = document.getElementById('topbar-title');
    const announcer = document.getElementById('page-announcer');
    if (topbarTitle) {
        topbarTitle.textContent = title;
    }
    if (announcer) {
        announcer.textContent = t('switchedTo', title);
    }
    document.title = `${title} - ModelProxy`;
}

function showPageLoading(visible) {
    document.getElementById('page-loading-bar')?.classList.toggle('hidden', !visible);
}

function formatTimezoneOffset(offsetMinutes) {
    const sign = offsetMinutes >= 0 ? '+' : '-';
    const absMinutes = Math.abs(offsetMinutes);
    const hours = String(Math.floor(absMinutes / 60)).padStart(2, '0');
    const minutes = String(absMinutes % 60).padStart(2, '0');
    return `${sign}${hours}:${minutes}`;
}

function formatServerDateTime(dateInput) {
    if (!dateInput) return '-';
    const date = dateInput instanceof Date ? dateInput : new Date(dateInput);
    if (Number.isNaN(date.getTime())) return '-';
    const shifted = new Date(date.getTime() + serverTimezoneOffsetMinutes * 60 * 1000);
    const year = shifted.getUTCFullYear();
    const month = String(shifted.getUTCMonth() + 1).padStart(2, '0');
    const day = String(shifted.getUTCDate()).padStart(2, '0');
    const hours = String(shifted.getUTCHours()).padStart(2, '0');
    const minutes = String(shifted.getUTCMinutes()).padStart(2, '0');
    const seconds = String(shifted.getUTCSeconds()).padStart(2, '0');
    return `${year}-${month}-${day} ${hours}:${minutes}:${seconds} UTC${serverTimezoneOffsetLabel}`;
}

function scheduleIdleTask(task) {
    if (typeof window.requestIdleCallback === 'function') {
        window.requestIdleCallback(task, { timeout: 1200 });
    } else {
        window.setTimeout(task, 300);
    }
}

function resolveUxVariant() {
    const searchParams = new URLSearchParams(window.location.search);
    const queryVariant = searchParams.get('ab');
    if (queryVariant === 'baseline' || queryVariant === 'optimized') {
        localStorage.setItem(UX_VARIANT_STORAGE_KEY, queryVariant);
        return queryVariant;
    }
    return localStorage.getItem(UX_VARIANT_STORAGE_KEY) || 'optimized';
}

function recordUxMetric(name, payload = {}) {
    try {
        const metrics = JSON.parse(localStorage.getItem(UX_METRICS_STORAGE_KEY) || '[]');
        metrics.push({
            name,
            variant: uxVariant,
            page: currentPage,
            timestamp: new Date().toISOString(),
            ...payload
        });
        localStorage.setItem(UX_METRICS_STORAGE_KEY, JSON.stringify(metrics.slice(-200)));
    } catch (_) {
    }
}

function prefetchNonCriticalAssets() {
    scheduleIdleTask(() => {
        ensureEcharts().catch(() => {});
    });
}

function ensureEcharts() {
    if (typeof window.echarts !== 'undefined') {
        return Promise.resolve(window.echarts);
    }
    if (echartsLoaderPromise) {
        return echartsLoaderPromise;
    }

    echartsLoaderPromise = new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = '/static/js/echarts.min.js?v=20260412-ui-ux';
        script.async = true;
        script.onload = () => resolve(window.echarts);
        script.onerror = () => reject(new Error(t('chartLoadFailed')));
        document.body.appendChild(script);
    });

    return echartsLoaderPromise;
}

function navigateTo(page) {
    if (!currentUser) {
        showLogin();
        return;
    }

    const normalizedPage = resolveAccessiblePage(page, currentUser);
    if (!normalizedPage) return;

    currentPage = normalizedPage;

    document.querySelectorAll('.nav-link').forEach(link => {
        const active = link.dataset.page === normalizedPage;
        link.classList.toggle('active', active);
        if (active) {
            link.setAttribute('aria-current', 'page');
        } else {
            link.removeAttribute('aria-current');
        }
    });

    document.querySelectorAll('.page:not(#login-page)').forEach(p => p.classList.add('hidden'));
    const pageEl = document.getElementById(`${normalizedPage}-page`);
    if (pageEl) {
        pageEl.classList.remove('hidden');
    }
    if (window.location.hash !== `#${normalizedPage}`) {
        window.location.hash = normalizedPage;
    }
    announcePage(normalizedPage);
    setSidebarOpen(false);
    document.getElementById('main')?.focus();

    loadPageData(normalizedPage);
}

async function loadPageData(page, options = {}) {
    const force = Boolean(options.force);
    const startedAt = performance.now();
    showPageLoading(true);
    try {
        switch (page) {
            case 'dashboard':
                await loadDashboard();
                break;
            case 'api-keys':
                await loadApiKeys();
                break;
            case 'usage':
                await loadUsage();
                break;
            case 'my-models':
                await loadMyModels();
                break;
            case 'users':
                if (currentUser?.role === 'admin') await loadUsers();
                break;
            case 'upstreams':
                if (currentUser?.role === 'admin') await loadUpstreams();
                break;
            case 'models':
                if (currentUser?.role === 'admin') await loadModels();
                break;
            case 'conditional-aliases':
                if (currentUser?.role === 'admin') await loadConditionalAliasesPage();
                break;
            case 'audit':
                if (currentUser?.role === 'admin') {
                    if (force) {
                        auditCurrentPage = 1;
                    }
                    await loadAuditFilters();
                    await loadAuditLogs();
                }
                break;
            case 'settings':
                await loadSettings();
                break;
            case 'other-links':
                await loadOtherLinks();
                break;
        }
    } finally {
        const duration = Math.round(performance.now() - startedAt);
        document.body.setAttribute('data-page-load-ms', String(duration));
        recordUxMetric('page_load', { page, duration });
        showPageLoading(false);
    }
}

async function handleLogin(e) {
    e.preventDefault();
    
    const username = document.getElementById('login-username').value;
    const password = document.getElementById('login-password').value;

    try {
        const response = await fetch(`${AUTH_BASE}/login`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ username, password })
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || t('loginFailed'));
        }

        const data = await response.json();
        localStorage.setItem('token', data.token);
        localStorage.setItem('user', JSON.stringify(data.user));
        currentUser = data.user;
        
        showApp();
        navigateTo(resolveAccessiblePage(getInitialPage(), currentUser));
        recordUxMetric('login_success', { username });
        showToast(t('loginSuccess'), 'success');
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function setRegisterPanelVisible(visible) {
    const panel = document.getElementById('register-form');
    const helpText = document.getElementById('register-help-text');
    const toggleBtn = document.getElementById('toggle-register-btn');
    if (!panel || !helpText || !toggleBtn) return;
    const shouldShow = Boolean(visible);
    panel.classList.toggle('hidden', !shouldShow);
    helpText.classList.toggle('hidden', !shouldShow);
    toggleBtn.textContent = shouldShow ? t('collapseRegister') : t('newUserRegister');
    toggleBtn.setAttribute('aria-expanded', shouldShow ? 'true' : 'false');
}

function toggleRegisterPanel() {
    const panel = document.getElementById('register-form');
    setRegisterPanelVisible(panel?.classList.contains('hidden'));
}

async function handleRegister(event) {
    event.preventDefault();
    const username = String(document.getElementById('register-username')?.value || '').trim();
    const fullName = String(document.getElementById('register-full-name')?.value || '').trim();
    const password = String(document.getElementById('register-password')?.value || '');
    const passwordConfirm = String(document.getElementById('register-password-confirm')?.value || '');

    if (!username) {
        showToast(t('usernameCannotEmpty'), 'error');
        return;
    }
    if (!fullName) {
        showToast(t('fullNameCannotEmpty'), 'error');
        return;
    }
    if (password.length < 6) {
        showToast(t('passwordMinLength'), 'error');
        return;
    }
    if (password !== passwordConfirm) {
        showToast(t('passwordMismatch'), 'error');
        return;
    }

    try {
        const response = await fetch(`${AUTH_BASE}/register`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                username,
                full_name: fullName,
                password
            })
        });
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
            throw new Error(data.error || t('registerSubmitFailed'));
        }
        document.getElementById('register-form')?.reset();
        setRegisterPanelVisible(false);
        showToast(data.message || t('registerSubmitted'), 'success');
    } catch (error) {
        showToast(error.message || t('registerSubmitFailed'), 'error');
    }
}

function handleLogout() {
    localStorage.removeItem('token');
    localStorage.removeItem('user');
    currentUser = null;
    recordUxMetric('logout');
    showLogin();
    showToast(t('loggedOut'), 'success');
}

async function api(endpoint, options = {}) {
    const token = localStorage.getItem('token');
    const sessionUserId = currentUser?.id || null;
    
    const response = await fetch(`${API_BASE}${endpoint}`, {
        ...options,
        headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${token}`,
            ...options.headers
        }
    });

    if (token !== localStorage.getItem('token') || sessionUserId !== (currentUser?.id || null)) {
        const staleSessionError = new Error(STALE_SESSION_MESSAGE);
        staleSessionError.code = STALE_SESSION_MESSAGE;
        throw staleSessionError;
    }

    if (response.status === 401) {
        handleLogout();
        throw new Error(t('sessionExpired'));
    }

    if (!response.ok) {
        const error = await response.json().catch(() => ({ error: t('requestFailed') }));
        throw new Error(error.error || t('requestFailed'));
    }

    if (response.status === 204 || response.headers.get('content-length') === '0') {
        return null;
    }

    return response.json();
}

async function loadDashboard() {
    try {
        if (currentUser?.role === 'admin') {
            try {
                const upstreams = await api('/upstreams');
                const tipEl = document.getElementById('no-upstream-tip');
                if (tipEl) {
                    tipEl.classList.toggle('hidden', !(!upstreams || upstreams.length === 0));
                }
            } catch (e) {
                const tipEl = document.getElementById('no-upstream-tip');
                if (tipEl) tipEl.classList.add('hidden');
            }
        } else {
            const tipEl = document.getElementById('no-upstream-tip');
            if (tipEl) tipEl.classList.add('hidden');
        }

        const usageUrl = currentUser?.role === 'admin' ? '/usage/all' : '/usage/me';
        const usage = await api(usageUrl);
        
        if (!usage) {
            showToast(t('getUsageDataFailed'), 'error');
            return;
        }
        
        const dailyUsage = Array.isArray(usage.daily_usage) ? usage.daily_usage : [];
        const today = formatServerDateInputValue(new Date());
        const todayUsage = dailyUsage.find(item => item.date === today);

        const todayRequests = todayUsage?.requests ?? usage.today_requests ?? 0;
        const todayTokens = todayUsage?.tokens ?? usage.today_tokens ?? 0;

        document.getElementById('stat-requests').textContent = Number(todayRequests).toLocaleString();
        document.getElementById('stat-tokens').textContent = Number(todayTokens).toLocaleString();

        const keysUrl = currentUser?.role === 'admin' ? '/keys/all' : '/keys';
        const keys = await api(keysUrl);
        document.getElementById('stat-keys').textContent = (keys?.total || 0).toLocaleString();

        if (currentUser?.role === 'admin') {
            document.getElementById('stat-daily-quota').textContent = '-';
            document.getElementById('stat-monthly-quota').textContent = '-';
        } else {
            const quota = await api('/usage/me/quota');
            const dailyLimitText = quota.daily_request_limit > 0 ? quota.daily_request_limit.toLocaleString() : t('unlimited');
            const monthlyLimitText = quota.monthly_request_limit > 0 ? quota.monthly_request_limit.toLocaleString() : t('unlimited');
            document.getElementById('stat-daily-quota').textContent = `${(quota.daily_request_used || 0).toLocaleString()} / ${dailyLimitText}`;
            document.getElementById('stat-monthly-quota').textContent = `${(quota.monthly_request_used || 0).toLocaleString()} / ${monthlyLimitText}`;
        }

        await renderDailyUsage(dailyUsage);
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function renderDailyUsage(data) {
    const chartContainer = document.getElementById('usage-chart');
    if (!chartContainer) return;
    chartContainer.setAttribute('aria-busy', 'true');
    chartContainer.textContent = t('loadingTrendChart');

    try {
        await ensureEcharts();
    } catch (error) {
        chartContainer.removeAttribute('aria-busy');
        chartContainer.textContent = t('trendChartLoadFailed');
        throw error;
    }

    const sortedData = Array.isArray(data)
        ? [...data].sort((a, b) => new Date(a.date) - new Date(b.date))
        : [];
    
    const dates = sortedData.map(item => item.date || '');
    const requests = sortedData.map(item => Number(item.requests || 0));

    if (usageChart) {
        usageChart.dispose();
    }
    chartContainer.textContent = '';
    usageChart = echarts.init(chartContainer);

    const option = {
        tooltip: {
            trigger: 'axis',
            axisPointer: {
                type: 'cross',
                label: {
                    backgroundColor: '#6a7985'
                }
            }
        },
        grid: {
            left: '3%',
            right: '4%',
            bottom: '3%',
            top: '5%',
            containLabel: true
        },
        xAxis: [
            {
                type: 'category',
                boundaryGap: false,
                data: dates,
                axisLabel: {
                    color: '#64748b'
                },
                axisLine: {
                    lineStyle: {
                        color: '#e2e8f0'
                    }
                }
            }
        ],
        yAxis: [
            {
                type: 'value',
                axisLabel: {
                    color: '#64748b'
                },
                splitLine: {
                    lineStyle: {
                        color: '#e2e8f0',
                        type: 'dashed'
                    }
                }
            }
        ],
        series: [
            {
                name: t('requests'),
                type: 'line',
                smooth: true,
                lineStyle: {
                    width: 3,
                    color: '#3b82f6'
                },
                showSymbol: false,
                areaStyle: {
                    opacity: 0.8,
                    color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                        {
                            offset: 0,
                            color: 'rgba(59, 130, 246, 0.5)'
                        },
                        {
                            offset: 1,
                            color: 'rgba(59, 130, 246, 0.05)'
                        }
                    ])
                },
                emphasis: {
                    focus: 'series'
                },
                data: requests
            }
        ]
    };

    if (dates.length === 0) {
        usageChart.clear();
        usageChart.setOption({
            title: {
                text: t('noTrendData'),
                left: 'center',
                top: 'center',
                textStyle: {
                    color: '#94a3b8',
                    fontSize: 14,
                    fontWeight: 'normal'
                }
            }
        });
    } else {
        usageChart.setOption(option);
    }
    chartContainer.removeAttribute('aria-busy');
}


async function loadApiKeys() {
    try {
        const url = currentUser?.role === 'admin' ? '/keys/all' : '/keys';
        const data = await api(url);
        
        const tbody = document.getElementById('api-keys-list');
        const isAdmin = currentUser?.role === 'admin';
        
        tbody.innerHTML = data.items.map(key => {
            const displayKey = formatDisplayKey(key.key_prefix, key.key_suffix);
            const copyValue = key.key_full || key.key_prefix || '';
            const encodedCopyValue = encodeURIComponent(copyValue);
            return `
            <tr>
                <td>${key.name}</td>
                <td><code style="user-select:none;">${escapeHtml(displayKey)}</code> <button class="btn btn-sm btn-secondary" onclick="copyEncodedText('${encodedCopyValue}')" title="${t('copy')}">${t('copy')}</button></td>
                ${isAdmin ? `<td>${key.username || '-'}</td>` : ''}
                <td><span class="status-${key.status}">${key.status}</span></td>
                <td>${key.rpm_limit || t('unlimited')}</td>
                <td>${key.tpm_limit || t('unlimited')}</td>
                <td>${formatServerDateTime(key.created_at)}</td>
                <td>
                    <button class="btn btn-sm btn-danger" onclick="deleteApiKey('${key.id}')">${t('delete')}</button>
                </td>
            </tr>`;
        }).join('');
    } catch (error) {
        showToast(error.message, 'error');
    }
}

let currentModelsList = [];

function formatModelAliasWithOriginal(modelName, modelAlias, originalModelName, routedModel) {
    const alias = modelAlias || modelName || '-';
    const original = originalModelName || modelName || '-';
    if (!routedModel || routedModel === alias || routedModel === original) {
        if (alias === original) {
            return `<code>${alias}</code>`;
        }
        return `<code>${alias}</code><div style="color:#1677ff;font-size:12px;"><code style="color:#1677ff;">${original}</code></div>`;
    }
    if (alias === original) {
        return `<code>${alias}</code><div style="color:#52c41a;font-size:12px;">${t('actualModel')}: <code style="color:#52c41a;">${routedModel}</code></div>`;
    }
    return `<code>${alias}</code><div style="color:#1677ff;font-size:12px;"><code style="color:#1677ff;">${original}</code></div><div style="color:#52c41a;font-size:12px;">${t('actualModel')}: <code style="color:#52c41a;">${routedModel}</code></div>`;
}

async function loadMyModels() {
    try {
        const data = await api('/models/my');
        currentModelsList = data || [];
        
        const container = document.getElementById('my-models-list');
        if (data && data.length > 0) {
            container.innerHTML = '<ul style="list-style: none; padding: 0;">' + 
                data.map((model, index) => {
                    const modelName = model.model_name || model;
                    const upstreamName = model.upstream_name || '';
                    const provider = model.provider || 'openai';
                    return `<li style="padding: 8px; border-bottom: 1px solid #eee; cursor: pointer;" onclick="selectModelForExample(${index})" class="model-item" data-index="${index}">
                        <code>${escapeHtml(modelName)}</code>
                        ${upstreamName ? `<small style="color: #666; margin-left: 8px;">(${upstreamName})</small>` : ''}
                        <span class="badge" style="margin-left: 8px; font-size: 0.7rem;">${PROVIDER_NAMES[provider] || provider}</span>
                    </li>`;
                }).join('') +
                '</ul>';
            
            // Set model name and provider in example code
            const firstModel = data[0];
            updateApiExamples(firstModel?.model_name || 'gpt-3.5-turbo', firstModel?.provider || 'openai');
            
            // Highlight first model
            highlightSelectedModel(0);
        } else {
            container.innerHTML = `<p>${t('noAvailableModels')}</p>`;
        }
        
        // Initialize tab switching
        setupApiExampleTabs();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function selectModelForExample(index) {
    const model = currentModelsList[index];
    if (model) {
        updateApiExamples(model.model_name, model.provider || 'openai');
        highlightSelectedModel(index);
    }
}

function highlightSelectedModel(index) {
    document.querySelectorAll('.model-item').forEach((item, i) => {
        if (i === index) {
            item.style.backgroundColor = '#e3f2fd';
            item.style.fontWeight = 'bold';
        } else {
            item.style.backgroundColor = 'transparent';
            item.style.fontWeight = 'normal';
        }
    });
}

function setupApiExampleTabs() {
    const tabBtns = document.querySelectorAll('#api-usage-examples .tab-btn');
    tabBtns.forEach(btn => {
        btn.addEventListener('click', () => {
            // Remove all active classes
            tabBtns.forEach(b => b.classList.remove('active'));
            document.querySelectorAll('#api-usage-examples .tab-content').forEach(c => c.classList.remove('active'));
            document.querySelectorAll('#api-usage-examples .tab-content').forEach(c => c.style.display = 'none');
            
            // Add active class to current button and content
            btn.classList.add('active');
            const tabId = btn.dataset.tab + '-example';
            const tabContent = document.getElementById(tabId);
            if (tabContent) {
                tabContent.classList.add('active');
                tabContent.style.display = 'block';
            }
        });
    });
}

function updateApiExamples(modelName, provider = 'openai') {
    const curlCode = document.getElementById('curl-code');
    const pythonCode = document.getElementById('python-code');
    const nodejsCode = document.getElementById('nodejs-code');
    
    // Update title to show current model and provider
    const exampleCard = document.getElementById('api-usage-examples');
    if (exampleCard) {
        const title = exampleCard.querySelector('h2');
        if (title) {
            title.innerHTML = `${t('apiExamples')} - <code>${modelName}</code> <span class="badge">${PROVIDER_NAMES[provider] || provider}</span>`;
        }
    }
    
    // Generate different example code based on provider
    if (provider === 'minimax') {
        // MiniMax format example
        if (curlCode) {
            curlCode.textContent = `# MiniMax API
curl ${systemBaseUrl}/chat/completions \\
  -H "Content-Type: application/json" \\
  -H "Authorization: Bearer YOUR_API_KEY" \\
  -d '{
    "model": "${modelName}",
    "messages": [
      {"role": "system", "name": "MiniMax AI", "content": "You are a helpful assistant"},
      {"role": "user", "name": "user", "content": "Hello"}
    ],
    "stream": true,
    "temperature": 0.1,
    "max_tokens": 20480
  }'`;
        }
        
        if (pythonCode) {
            pythonCode.textContent = `# MiniMax Python
import openai

client = openai.OpenAI(
    api_key="YOUR_API_KEY", # Replace with your API key
    base_url="${systemBaseUrl}"
)

# Streaming
stream = client.chat.completions.create(
    model="${modelName}",
    messages=[
        {"role": "system", "name": "MiniMax AI", "content": "You are a helpful assistant"},
        {"role": "user", "name": "user", "content": "Hello"}
    ],
    stream=True,
    temperature=0.1,
    max_tokens=20480
)

for chunk in stream:
    if chunk.choices[0].delta.content:
        print(chunk.choices[0].delta.content, end="")`;
        }
        
        if (nodejsCode) {
            nodejsCode.textContent = `// MiniMax Node.js
const OpenAI = require('openai');

const client = new OpenAI({
    apiKey: 'YOUR_API_KEY', // Replace with your API key
    baseURL: '${systemBaseUrl}'
});

async function chat() {
    const stream = await client.chat.completions.create({
        model: '${modelName}',
        messages: [
            { role: 'system', name: 'MiniMax AI', content: 'You are a helpful assistant' },
            { role: 'user', name: 'user', content: 'Hello' }
        ],
        stream: true,
        temperature: 0.1,
        max_tokens: 20480
    });
    
    for await (const chunk of stream) {
        process.stdout.write(chunk.choices[0]?.delta?.content || '');
    }
}

chat();`;
        }
    } else {
        // Standard OpenAI format (openai, azure, anthropic, vllm, sglang, etc.)
        if (curlCode) {
            curlCode.textContent = `# OpenAI API
curl ${systemBaseUrl}/chat/completions \\
  -H "Content-Type: application/json" \\
  -H "Authorization: Bearer YOUR_API_KEY" \\
  -d '{
    "model": "${modelName}",
    "messages": [{"role": "user", "content": "Hello!"}],
    "temperature": 0.1,
    "max_tokens": 20480
  }'`;
        }
        
        if (pythonCode) {
            pythonCode.textContent = `# OpenAI Python
import openai

client = openai.OpenAI(
    api_key="YOUR_API_KEY", # Replace with your API key
    base_url="${systemBaseUrl}"
)

response = client.chat.completions.create(
    model="${modelName}",
    messages=[{"role": "user", "content": "Hello!"}],
    temperature=0.1,
    max_tokens=20480
)

print(response.choices[0].message.content)`;
        }
        
        if (nodejsCode) {
            nodejsCode.textContent = `// OpenAI Node.js
const OpenAI = require('openai');

const client = new OpenAI({
    apiKey: 'YOUR_API_KEY', // Replace with your API key
    baseURL: '${systemBaseUrl}'
});

async function chat() {
    const response = await client.chat.completions.create({
        model: '${modelName}',
        messages: [{ role: 'user', content: 'Hello!' }],
        temperature: 0.1,
        max_tokens: 20480
    });
    console.log(response.choices[0].message.content);
}

chat();`;
        }
    }
}

async function showCreateKeyModal() {
    showModal(t('createKey'), `
        <form id="create-key-form">
            <div class="form-group">
                <label>${t('name')}</label>
                <input type="text" name="name" required>
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('create')}</button>
        </form>
    `);

    document.getElementById('create-key-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const formData = new FormData(e.target);
        
        try {
            const result = await api('/keys', {
                method: 'POST',
                body: JSON.stringify({
                    name: formData.get('name'),
                    rpm_limit: 0,
                    tpm_limit: 0,
                    daily_limit: 0,
                    user_id: currentUser.id
                })
            });

            hideModal();
            showToast(t('keyCreateSuccess'), 'success');
            const encodedNewKey = encodeURIComponent(result.key || '');
            const maskedNewKey = maskApiKey(result.key);
            
            showModal(t('keyCreated'), `
                <p>${t('saveKeyHint')}</p>
                <div class="key-secret" style="position:relative;"><code>${escapeHtml(maskedNewKey)}</code> <button class="btn btn-sm btn-secondary" onclick="copyEncodedText('${encodedNewKey}')">${t('copyKey')}</button></div>
            `);
            
            loadApiKeys();
        } catch (error) {
            showToast(error.message, 'error');
        }
    });
}

async function deleteApiKey(id) {
    if (!confirm(t('confirmDeleteKey'))) return;
    
    try {
        await api(`/keys/${id}`, { method: 'DELETE' });
        showToast(t('keyDeleted'), 'success');
        loadApiKeys();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function loadUsage() {
    try {
        const startDate = document.getElementById('usage-start-date')?.value;
        const endDate = document.getElementById('usage-end-date')?.value;
        
        let url = currentUser?.role === 'admin' ? '/usage/all' : '/usage/me';
        const params = new URLSearchParams();
        if (startDate) params.append('start_time', buildServerDateBoundaryIso(startDate, false));
        if (endDate) params.append('end_time', buildServerDateBoundaryIso(endDate, true));
        if (params.toString()) url += `?${params.toString()}`;

        const data = await api(url);
        
        if (!data) {
            showToast(t('getUsageDataFailed'), 'error');
            return;
        }
        
        document.getElementById('usage-requests').textContent = (data.total_requests || 0).toLocaleString();
        document.getElementById('usage-tokens').textContent = (data.total_tokens || 0).toLocaleString();

        const dailyTbody = document.getElementById('daily-usage-list');
        if (data.daily_usage && data.daily_usage.length > 0) {
            dailyTbody.innerHTML = data.daily_usage.map(item => `
                <tr>
                    <td>${item.date}</td>
                    <td>${item.requests.toLocaleString()}</td>
                    <td>${item.tokens.toLocaleString()}</td>
                </tr>
            `).join('');
        } else {
            dailyTbody.innerHTML = `<tr><td colspan="3">${t('noData')}</td></tr>`;
        }

        const modelTbody = document.getElementById('model-usage-list');
        if (data.model_usage && data.model_usage.length > 0) {
            modelTbody.innerHTML = data.model_usage.map(item => `
                <tr>
                    <td>${item.model}</td>
                    <td>${item.requests.toLocaleString()}</td>
                    <td>${item.tokens.toLocaleString()}</td>
                </tr>
            `).join('');
        } else {
            modelTbody.innerHTML = `<tr><td colspan="3">${t('noData')}</td></tr>`;
        }

        const upstreamModelTbody = document.getElementById('upstream-model-usage-list');
        if (data.upstream_model_usage && data.upstream_model_usage.length > 0) {
            upstreamModelTbody.innerHTML = data.upstream_model_usage.map(item => `
                <tr>
                    <td>${item.model}</td>
                    <td>${item.requests.toLocaleString()}</td>
                    <td>${item.tokens.toLocaleString()}</td>
                </tr>
            `).join('');
        } else {
            upstreamModelTbody.innerHTML = `<tr><td colspan="3">${t('noData')}</td></tr>`;
        }

        const userCard = document.getElementById('user-usage-card');
        const userTbody = document.getElementById('user-usage-list');
        
        if (currentUser?.role === 'admin' && data.user_usage) {
            if (userCard) userCard.classList.remove('hidden');
            const sortedUsers = [...data.user_usage].sort((a, b) => b.tokens - a.tokens);
            const maxTokens = sortedUsers.length > 0 ? Math.max(...sortedUsers.map(item => Number(item.tokens || 0)), 1) : 1;
            
            if (sortedUsers.length > 0) {
                userTbody.innerHTML = sortedUsers.map((item, index) => {
                    const tokens = Number(item.tokens || 0);
                    const ratio = Math.max((tokens / maxTokens) * 100, 4);
                    const shortId = (item.user_id || '').substring(0, 8);
                    const rankClass = index < 3 ? 'usage-rank-badge usage-rank-top' : 'usage-rank-badge';
                    return `
                    <tr>
                        <td><span class="${rankClass}">#${index + 1}</span></td>
                        <td>
                            <div class="usage-user-cell">
                                <span class="usage-user-name">${item.username || 'Unknown'}</span>
                                <span class="usage-user-id">${shortId ? `${shortId}...` : '-'}</span>
                            </div>
                        </td>
                        <td>${item.requests.toLocaleString()}</td>
                        <td>
                            <div class="usage-token-wrap">
                                <div class="usage-token-bar">
                                    <div class="usage-token-fill" style="width: ${ratio}%;"></div>
                                </div>
                                <span class="usage-token-text">${tokens.toLocaleString()}</span>
                            </div>
                        </td>
                    </tr>
                `}).join('');
            } else {
                userTbody.innerHTML = `<tr><td colspan="4">${t('noData')}</td></tr>`;
            }
        } else {
            if (userCard) userCard.classList.add('hidden');
        }
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function loadUsers() {
    try {
        const [data, pending] = await Promise.all([
            api('/users'),
            loadPendingRegistrations()
        ]);
        
        const tbody = document.getElementById('users-list');
        tbody.innerHTML = data.items.map(user => {
            const isAdmin = user.role === 'admin';
            return `
            <tr>
                <td>${user.username}</td>
                <td>${user.email}</td>
                <td><span class="role-${user.role}">${user.role}</span></td>
                <td><span class="status-${user.status}">${user.status}</span></td>
                <td>${user.quota_used.toLocaleString()} / ${user.quota_limit.toLocaleString()}</td>
                <td>${(user.daily_request_limit || 0).toLocaleString()}</td>
                <td>${(user.monthly_request_limit || 0).toLocaleString()}</td>
                <td>${formatServerDateTime(user.created_at)}</td>
                <td>
                    <button class="btn btn-sm btn-primary" onclick="event.stopPropagation(); editUser('${user.id}')">${t('edit')}</button>
                    ${!isAdmin ? `<button class="btn btn-sm btn-danger" onclick="event.stopPropagation(); deleteUser('${user.id}', '${user.username}')">${t('delete')}</button>` : ''}
                </td>
            </tr>
        `}).join('');

        return { users: data, pending };
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function loadPendingRegistrations() {
    const tbody = document.getElementById('pending-registrations-list');
    if (!tbody || currentUser?.role !== 'admin') {
        return null;
    }
    try {
        const data = await api('/users/pending-registrations');
        if (!data.items || data.items.length === 0) {
            tbody.innerHTML = `<tr><td colspan="4">${t('noPendingRegistrations')}</td></tr>`;
            return data;
        }
        tbody.innerHTML = data.items.map(item => `
            <tr>
                <td>${escapeHtml(item.username)}</td>
                <td>${escapeHtml(item.full_name)}</td>
                <td>${formatServerDateTime(item.submitted_at)}</td>
                <td>
                    <button class="btn btn-sm btn-primary" onclick="approvePendingRegistration('${item.id}')">${t('approve')}</button>
                    <button class="btn btn-sm btn-danger" onclick="rejectPendingRegistration('${item.id}', '${escapeHtml(item.username)}')">${t('reject')}</button>
                </td>
            </tr>
        `).join('');
        return data;
    } catch (error) {
        tbody.innerHTML = `<tr><td colspan="4">${t('loadPendingFailed')}</td></tr>`;
        throw error;
    }
}

async function approvePendingRegistration(id) {
    if (!confirm(t('confirmApproveRegistration'))) return;
    try {
        await api(`/users/pending-registrations/${id}/approve`, { method: 'POST' });
        showToast(t('registrationApproved'), 'success');
        await loadUsers();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function rejectPendingRegistration(id, username) {
    if (!confirm(t('confirmRejectRegistration', username))) return;
    try {
        await api(`/users/pending-registrations/${id}`, { method: 'DELETE' });
        showToast(t('registrationRejected'), 'success');
        await loadPendingRegistrations();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function isValidHttpUrl(value) {
    const text = String(value || '').trim();
    if (!text) return false;
    try {
        const parsed = new URL(text);
        return parsed.protocol === 'http:' || parsed.protocol === 'https:';
    } catch (_) {
        return false;
    }
}

function openOtherLink(encodedUrl) {
    const url = decodeURIComponent(encodedUrl || '');
    if (!isValidHttpUrl(url)) {
        showToast(t('invalidUrlFormat'), 'error');
        return;
    }
    window.open(url, '_blank', 'noopener,noreferrer');
}

async function loadOtherLinks() {
    const tbody = document.getElementById('other-links-list');
    if (!tbody) return;
    try {
        const data = await api('/other-links');
        if (!Array.isArray(data) || data.length === 0) {
            tbody.innerHTML = `<tr><td colspan="4">${t('noOtherLinks')}</td></tr>`;
            return;
        }
        const isAdmin = currentUser?.role === 'admin';
        tbody.innerHTML = data.map(item => {
            const encodedName = encodeURIComponent(item.name || '');
            const encodedUrl = encodeURIComponent(item.url || '');
            const actions = isAdmin
                ? `<button class="btn btn-sm btn-primary" onclick="showEditOtherLinkModal('${item.id}', '${encodedName}', '${encodedUrl}')">${t('edit')}</button>
                   <button class="btn btn-sm btn-danger" onclick="deleteOtherLink('${item.id}', '${encodedName}')">${t('delete')}</button>`
                : '-';
            return `
                <tr>
                    <td>${escapeHtml(item.name || '-')}</td>
                    <td>
                        <a href="${escapeHtml(item.url || '#')}" target="_blank" rel="noopener noreferrer">${escapeHtml(item.url || '-')}</a>
                    </td>
                    <td>${formatServerDateTime(item.updated_at || item.created_at)}</td>
                    <td class="${isAdmin ? '' : 'admin-only hidden'}">
                        <button class="btn btn-sm btn-outline" onclick="openOtherLink('${encodedUrl}')">${t('open')}</button>
                        ${isAdmin ? actions : ''}
                    </td>
                </tr>
            `;
        }).join('');
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function showCreateOtherLinkModal() {
    if (currentUser?.role !== 'admin') {
        showToast(t('adminOnly'), 'error');
        return;
    }
    showModal(t('addOtherLink'), `
        <form id="other-link-form">
            <div class="form-group">
                <label>${t('name')}</label>
                <input type="text" name="name" placeholder="e.g.: Internal Docs" required>
            </div>
            <div class="form-group">
                <label>${t('linkUrl')}</label>
                <input type="url" name="url" placeholder="https://example.com/docs" required>
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('save')}</button>
        </form>
    `);
    bindOtherLinkFormSubmit('POST', null);
}

function showEditOtherLinkModal(id, encodedName, encodedUrl) {
    if (currentUser?.role !== 'admin') {
        showToast(t('adminOnly'), 'error');
        return;
    }
    const name = decodeURIComponent(encodedName || '');
    const url = decodeURIComponent(encodedUrl || '');
    showModal(t('editOtherLink'), `
        <form id="other-link-form">
            <div class="form-group">
                <label>${t('name')}</label>
                <input type="text" name="name" value="${escapeHtml(name)}" required>
            </div>
            <div class="form-group">
                <label>${t('linkUrl')}</label>
                <input type="url" name="url" value="${escapeHtml(url)}" required>
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('save')}</button>
        </form>
    `);
    bindOtherLinkFormSubmit('PUT', id);
}

function bindOtherLinkFormSubmit(method, id) {
    const form = document.getElementById('other-link-form');
    if (!form) return;
    form.onsubmit = async (event) => {
        event.preventDefault();
        const formData = new FormData(form);
        const name = String(formData.get('name') || '').trim();
        const url = String(formData.get('url') || '').trim();

        if (!name) {
            showToast(t('nameCannotEmpty'), 'error');
            return;
        }
        if (!isValidHttpUrl(url)) {
            showToast(t('urlMustBeHttp'), 'error');
            return;
        }

        try {
            const endpoint = method === 'POST' ? '/other-links' : `/other-links/${id}`;
            await api(endpoint, {
                method,
                body: JSON.stringify({ name, url })
            });
            hideModal();
            showToast(t('linkSaved'), 'success');
            await loadOtherLinks();
        } catch (error) {
            showToast(error.message, 'error');
        }
    };
}

async function deleteOtherLink(id, encodedName) {
    const name = decodeURIComponent(encodedName || '');
    if (!confirm(t('confirmDeleteLink', name))) return;
    try {
        await api(`/other-links/${id}`, { method: 'DELETE' });
        showToast(t('linkDeleted'), 'success');
        await loadOtherLinks();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function showCreateUserModal() {
    showModal(t('createUserTitle'), `
        <form id="create-user-form">
            <div class="form-group">
                <label>${t('username')}</label>
                <input type="text" name="username" required>
            </div>
            <div class="form-group">
                <label>${t('email')}</label>
                <input type="email" name="email" required>
            </div>
            <div class="form-group">
                <label>${t('password')}</label>
                <input type="password" name="password" required>
            </div>
            <div class="form-group">
                <label>${t('role')}</label>
                <select name="role">
                    <option value="user">${t('ordinaryUser')}</option>
                    <option value="admin">${t('admin')}</option>
                </select>
            </div>
            <div class="form-group">
                <label>${t('dailyRequestQuota')}</label>
                <input type="number" name="daily_request_limit" value="0" min="0">
            </div>
            <div class="form-group">
                <label>${t('monthlyRequestQuota')}</label>
                <input type="number" name="monthly_request_limit" value="0" min="0">
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('create')}</button>
        </form>
    `);

    document.getElementById('create-user-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const formData = new FormData(e.target);
        
        try {
            await api('/users', {
                method: 'POST',
                body: JSON.stringify({
                    username: formData.get('username'),
                    email: formData.get('email'),
                    password: formData.get('password'),
                    role: formData.get('role'),
                    daily_request_limit: parseInt(formData.get('daily_request_limit') || '0', 10),
                    monthly_request_limit: parseInt(formData.get('monthly_request_limit') || '0', 10)
                })
            });

            hideModal();
            showToast(t('userCreated'), 'success');
            loadUsers();
        } catch (error) {
            showToast(error.message, 'error');
        }
    });
}

async function editUser(id) {
    try {
        const user = await api(`/users/${id}`);
        showModal(t('editUser'), `
            <form id="edit-user-form">
                <div class="form-group">
                    <label>${t('username')}</label>
                    <input type="text" name="username" value="${user.username}" required>
                </div>
                <div class="form-group">
                    <label>${t('email')}</label>
                    <input type="email" name="email" value="${user.email}" required>
                </div>
                <div class="form-group">
                    <label>${t('role')}</label>
                    <select name="role">
                        <option value="user" ${user.role === 'user' ? 'selected' : ''}>${t('ordinaryUser')}</option>
                        <option value="admin" ${user.role === 'admin' ? 'selected' : ''}>${t('admin')}</option>
                    </select>
                </div>
                <div class="form-group">
                    <label>${t('status')}</label>
                    <select name="status">
                        <option value="active" ${user.status === 'active' ? 'selected' : ''}>active</option>
                        <option value="disabled" ${user.status === 'disabled' ? 'selected' : ''}>disabled</option>
                    </select>
                </div>
                <div class="form-group">
                    <label>${t('dailyRequestQuota')}</label>
                    <input type="number" name="daily_request_limit" value="${user.daily_request_limit || 0}" min="0">
                </div>
                <div class="form-group">
                    <label>${t('monthlyRequestQuota')}</label>
                    <input type="number" name="monthly_request_limit" value="${user.monthly_request_limit || 0}" min="0">
                </div>
                <button type="submit" class="btn btn-primary btn-block">${t('save')}</button>
            </form>
        `);

        document.getElementById('edit-user-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const formData = new FormData(e.target);
            try {
                await api(`/users/${id}`, {
                    method: 'PUT',
                    body: JSON.stringify({
                        username: formData.get('username'),
                        email: formData.get('email'),
                        role: formData.get('role'),
                        status: formData.get('status'),
                        daily_request_limit: parseInt(formData.get('daily_request_limit') || '0', 10),
                        monthly_request_limit: parseInt(formData.get('monthly_request_limit') || '0', 10)
                    })
                });
                hideModal();
                showToast(t('userUpdated'), 'success');
                loadUsers();
            } catch (error) {
                showToast(error.message, 'error');
            }
        });
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function deleteUser(id, username) {
    if (!confirm(t('confirmDeleteUser', username))) return;
    
    try {
        await api(`/users/${id}`, { method: 'DELETE' });
        showToast(t('userDeleted'), 'success');
        loadUsers();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function loadUpstreams() {
    try {
        const data = await api('/upstreams');
        
        const tbody = document.getElementById('upstreams-list');
        if (!data || data.length === 0) {
            tbody.innerHTML = `<tr><td colspan="7">${t('noUpstreamAvailable')}</td></tr>`;
            return;
        }
        tbody.innerHTML = data.map(upstream => `
            <tr>
                <td>${upstream.name}</td>
                <td>${API_TYPE_NAMES[upstream.api_type] || upstream.api_type || '-'}</td>
                <td>${upstream.api_key_masked ? `<code style="user-select:none;">${escapeHtml(upstream.api_key_masked)}</code> <button class="btn btn-sm btn-secondary" onclick="copyUpstreamApiKey('${upstream.id}')" title="${t('copy')}">${t('copy')}</button>` : '-'}</td>
                <td><span class="status-${upstream.status}">${upstream.status}</span></td>
                <td>${upstream.daily_request_limit || 2000}</td>
                <td>${upstream.monthly_request_limit || 50000}</td>
                <td>
                    <button class="btn btn-sm btn-primary" onclick="editUpstream('${upstream.id}')">${t('edit')}</button>
                    <button class="btn btn-sm btn-danger" onclick="deleteUpstream('${upstream.id}')">${t('delete')}</button>
                </td>
            </tr>
        `).join('');
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function copyUpstreamApiKey(upstreamId) {
    try {
        const result = await api(`/upstreams/${upstreamId}/api-key`);
        if (result.api_key) {
            await navigator.clipboard.writeText(result.api_key);
            showToast(t('copiedToClipboard'), 'success');
        } else {
            showToast(t('noApiKeyToCopy'), 'warning');
        }
    } catch (error) {
        showToast(t('copyFailed'), 'error');
    }
}

let allUsersList = [];
let upstreamModelsList = [];
let conditionalAliasList = [];

async function loadModels() {
    try {
        const usersRes = await api('/users');
        allUsersList = usersRes.items || [];
        
        const visibilityRes = await api('/models');
        const visibilityList = visibilityRes || [];
        const data = await api('/models/fetch');
        upstreamModelsList = mergeUpstreamModelsWithVisibility(data || [], visibilityList);
        renderManualModelUpstreamOptions();
        
        renderModelsList(upstreamModelsList, visibilityList);
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function mergeUpstreamModelsWithVisibility(upstreamsWithModels, visibilityList) {
    const modelMap = new Map();
    for (const visibility of (visibilityList || [])) {
        const key = visibility.upstream_id;
        if (!modelMap.has(key)) {
            modelMap.set(key, []);
        }
        modelMap.get(key).push(visibility.original_model_name || visibility.model_name);
    }
    return (upstreamsWithModels || []).map((upstream) => {
        const merged = new Set((upstream.models || []).filter(Boolean));
        for (const modelName of (modelMap.get(upstream.upstream_id) || [])) {
            if (modelName) merged.add(modelName);
        }
        return { ...upstream, models: Array.from(merged) };
    });
}

function renderManualModelUpstreamOptions() {
    const select = document.getElementById('manual-model-upstream-select');
    if (!select) return;
    if (!Array.isArray(upstreamModelsList) || upstreamModelsList.length === 0) {
        select.innerHTML = `<option value="">${t('noUpstreamAvailable')}</option>`;
        return;
    }
    select.innerHTML = upstreamModelsList
        .map(upstream => `<option value="${upstream.upstream_id}">${escapeHtml(upstream.upstream_name)}</option>`)
        .join('');
}

async function loadConditionalAliasesPage() {
    try {
        const usersRes = await api('/users');
        allUsersList = usersRes.items || [];
        const upstreamRes = await api('/upstreams');
        const allUpstreams = Array.isArray(upstreamRes) ? upstreamRes : [];
        const visibilityRes = await api('/models');
        const visibilityList = visibilityRes || [];
        const data = await api('/models/fetch');
        const mergedModels = mergeUpstreamModelsWithVisibility(data || [], visibilityList);
        const conditionalRes = await api('/models/conditional-aliases');
        conditionalAliasList = Array.isArray(conditionalRes) ? conditionalRes : [];
        upstreamModelsList = mergeUpstreamsForConditionalAliases(allUpstreams, mergedModels, conditionalAliasList);
        renderConditionalAliasList();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function mergeUpstreamsForConditionalAliases(allUpstreams, mergedModels, aliasList) {
    const byId = new Map();

    for (const upstream of (allUpstreams || [])) {
        byId.set(upstream.id, {
            upstream_id: upstream.id,
            upstream_name: upstream.name,
            models: []
        });
    }

    for (const upstream of (mergedModels || [])) {
        const existing = byId.get(upstream.upstream_id) || {
            upstream_id: upstream.upstream_id,
            upstream_name: upstream.upstream_name || upstream.upstream_id,
            models: []
        };
        if (!existing.upstream_name && upstream.upstream_name) {
            existing.upstream_name = upstream.upstream_name;
        }
        const models = new Set(existing.models || []);
        for (const model of (upstream.models || [])) {
            if (model) models.add(model);
        }
        existing.models = Array.from(models);
        byId.set(upstream.upstream_id, existing);
    }

    for (const alias of (aliasList || [])) {
        const appendModel = (upstreamId, modelName) => {
            if (!upstreamId || !modelName) return;
            const existing = byId.get(upstreamId) || {
                upstream_id: upstreamId,
                upstream_name: upstreamId,
                models: []
            };
            const models = new Set(existing.models || []);
            models.add(modelName);
            existing.models = Array.from(models);
            byId.set(upstreamId, existing);
        };
        for (const rule of (alias.rules || [])) {
            appendModel(rule.upstream_id, rule.model_name);
        }
        appendModel(alias?.fallback?.upstream_id, alias?.fallback?.model_name);
    }

    return Array.from(byId.values());
}

function renderModelsList(upstreamsWithModels, visibilityList) {
    const tbody = document.getElementById('models-list');
    let html = '';
    
    for (const upstream of upstreamModelsList) {
        for (const model of upstream.models) {
            const vis = visibilityList.find(v => v.upstream_id === upstream.upstream_id && v.model_name === model);
            const allUsersVisible = vis?.all_users_visible || false;
            const modelAliases = Array.isArray(vis?.model_aliases)
                ? vis.model_aliases
                : (vis?.model_alias ? vis.model_alias.split(',').map(s => s.trim()).filter(Boolean) : []);
            const modelAliasText = modelAliases.join(', ');
            const modelHeaders = vis?.model_headers && typeof vis.model_headers === 'object' ? vis.model_headers : {};
            const modelHeaderCount = Object.keys(modelHeaders).length;
            const retrySummary = formatModelRetrySummary(vis);
            const retryFailureAction = formatModelRetryFailureAction(vis);
            const allowedUsers = vis?.allowed_users || [];
            const encodedModel = encodeURIComponent(model);
            const aliasInputId = `model-alias-${upstream.upstream_id}-${encodedModel}`;
            
            html += `
                <tr>
                    <td>${upstream.upstream_name}</td>
                    <td><code>${model}</code></td>
                    <td>
                        <div style="display:flex; gap:6px; align-items:center;">
                            <input id="${aliasInputId}" type="text" value="${modelAliasText}" placeholder="${t('aliasPlaceholder')}" style="min-width:260px;">
                            <button class="btn btn-sm btn-primary" onclick="saveModelAlias('${upstream.upstream_id}', '${encodedModel}', '${aliasInputId}')">${t('saveAlias')}</button>
                        </div>
                    </td>
                    <td>${modelHeaderCount > 0 ? t('headerCount', modelHeaderCount) : '-'}</td>
                    <td>${escapeHtml(retrySummary)}</td>
                    <td>${escapeHtml(retryFailureAction)}</td>
                    <td>
                        <input type="checkbox" ${allUsersVisible ? 'checked' : ''} 
                            onchange="toggleAllUsers('${upstream.upstream_id}', '${model}', this.checked)">
                    </td>
                    <td>${allowedUsers.length > 0 ? t('userModelCount', allowedUsers.length) : '-'}</td>
                    <td>
                        <button class="btn btn-sm btn-primary" onclick="editModelVisibility('${upstream.upstream_id}', '${model}')">${t('configure')}</button>
                        <button class="btn btn-sm btn-secondary" onclick="testModelInModelPage('${upstream.upstream_id}', '${encodeURIComponent(model)}')">${t('test')}</button>
                    </td>
                </tr>
            `;
        }
    }
    
    tbody.innerHTML = html || `<tr><td colspan="9">${t('noModelData')}</td></tr>`;
}

function renderConditionalAliasList() {
    const tbody = document.getElementById('conditional-alias-list');
    if (!tbody) return;
    if (!Array.isArray(conditionalAliasList) || conditionalAliasList.length === 0) {
        tbody.innerHTML = `<tr><td colspan="6">${t('noConditionalAliases')}</td></tr>`;
        return;
    }

    const html = conditionalAliasList.map(item => {
        const fallbackUpstream = findUpstreamName(item?.fallback?.upstream_id);
        const fallbackModel = item?.fallback?.model_name || '-';
        const ruleCount = Array.isArray(item?.rules) ? item.rules.length : 0;
        const alias = item?.alias || '';
        const visibility = item?.all_users_visible ? t('allUsers') : t('userCount', (item?.user_ids || []).length);
        return `
            <tr>
                <td><code>${alias}</code></td>
                <td>${ruleCount}</td>
                <td>${fallbackUpstream}</td>
                <td><code>${fallbackModel}</code></td>
                <td>${visibility}</td>
                <td>
                    <button class="btn btn-sm btn-primary" data-action="edit-conditional-alias" data-alias="${encodeURIComponent(alias)}">${t('rules')}</button>
                    <button class="btn btn-sm btn-secondary" data-action="edit-conditional-alias-visibility" data-alias="${encodeURIComponent(alias)}">${t('visibility')}</button>
                    <button class="btn btn-sm btn-danger" data-action="delete-conditional-alias" data-alias="${encodeURIComponent(alias)}">${t('delete')}</button>
                </td>
            </tr>
        `;
    }).join('');
    tbody.innerHTML = html;
}

function findUpstreamName(upstreamId) {
    if (!upstreamId) return '-';
    const target = upstreamModelsList.find(u => u.upstream_id === upstreamId);
    return target?.upstream_name || upstreamId;
}

function collectAllModelOptions() {
    const options = [];
    for (const upstream of upstreamModelsList) {
        const upstreamId = upstream.upstream_id;
        for (const model of (upstream.models || [])) {
            options.push({
                upstream_id: upstreamId,
                upstream_name: upstream.upstream_name,
                model_name: model
            });
        }
    }
    return options;
}

function escapeHtml(value) {
    return String(value ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function encodeRouteValue(upstreamId, modelName) {
    return encodeURIComponent(JSON.stringify({ upstream_id: upstreamId, model_name: modelName }));
}

function decodeRouteValue(encodedValue) {
    try {
        const parsed = JSON.parse(decodeURIComponent(encodedValue || ''));
        return {
            upstream_id: parsed?.upstream_id || '',
            model_name: parsed?.model_name || ''
        };
    } catch (_) {
        return { upstream_id: '', model_name: '' };
    }
}

function buildRouteTargetOptions(modelOptions, selectedUpstreamId, selectedModelName) {
    const selectedValue = encodeRouteValue(selectedUpstreamId, selectedModelName);
    const hasSelected = modelOptions.some(
        (opt) => opt.upstream_id === selectedUpstreamId && opt.model_name === selectedModelName
    );
    let html = modelOptions.map((opt) => {
        const value = encodeRouteValue(opt.upstream_id, opt.model_name);
        const selected = value === selectedValue ? 'selected' : '';
        return `<option value="${value}" ${selected}>${escapeHtml(opt.upstream_name)} / ${escapeHtml(opt.model_name)}</option>`;
    }).join('');
    if (!hasSelected && selectedUpstreamId && selectedModelName) {
        html = `<option value="${selectedValue}" selected>${escapeHtml(selectedUpstreamId)} / ${escapeHtml(selectedModelName)}${t('unlimited')}</option>` + html;
    }
    return html;
}

function getModelRetrySettings(vis) {
    return {
        retry_count: Number.isInteger(vis?.retry_count) ? vis.retry_count : 0,
        retry_interval_seconds: Number.isInteger(vis?.retry_interval_seconds) ? vis.retry_interval_seconds : 0,
        retry_backoff_strategy: (vis?.retry_backoff_strategy || 'fixed').toString(),
        retry_max_interval_seconds: Number.isInteger(vis?.retry_max_interval_seconds) ? vis.retry_max_interval_seconds : 0,
        retry_failure_strategy: (vis?.retry_failure_strategy || 'error').toString(),
        retry_fallback_upstream_id: vis?.retry_fallback_upstream_id || null,
        retry_fallback_model_name: vis?.retry_fallback_model_name || null
    };
}

function formatModelRetrySummary(vis) {
    const settings = getModelRetrySettings(vis);
    if (settings.retry_count <= 0) {
        return t('closed');
    }
    const strategyMap = {
        fixed: t('fixed'),
        exponential: t('exponential'),
        exponential_jitter: t('exponentialJitterShort')
    };
    const strategyLabel = strategyMap[settings.retry_backoff_strategy] || t('fixed');
    const capText = settings.retry_max_interval_seconds > 0 ? ` / ${t('max')} ${settings.retry_max_interval_seconds} ms` : '';
    return `${settings.retry_count} ${t('times')} / ${settings.retry_interval_seconds} ms / ${strategyLabel}${capText}`;
}

function formatModelRetryFailureAction(vis) {
    const settings = getModelRetrySettings(vis);
    if (settings.retry_failure_strategy === 'route') {
        const upstreamName = findUpstreamName(settings.retry_fallback_upstream_id);
        const modelName = settings.retry_fallback_model_name || '-';
        return `${t('routeTo')} ${upstreamName} / ${modelName}`;
    }
    return t('returnError');
}

function renderConditionalAliasRuleRows(rules, modelOptions) {
    return rules.map((rule, idx) => {
        const optionsHtml = buildRouteTargetOptions(modelOptions, rule.upstream_id, rule.model_name);
        const keywordsText = Array.isArray(rule.keywords) ? rule.keywords.join(', ') : '';
        const minVal = rule.min_input_tokens ?? '';
        const maxVal = rule.max_input_tokens ?? '';
        const startTime = formatRuleTimeValue(rule.start_time);
        const endTime = formatRuleTimeValue(rule.end_time);
        const conditionType = rule.condition_type || 'token_gt';
        return `
            <div class="conditional-rule-row" data-index="${idx}" style="border:1px solid #e5e7eb;border-radius:8px;padding:10px;margin-bottom:10px;">
                <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                    <strong>${t('condition', idx + 1)}</strong>
                    <button type="button" class="btn btn-sm btn-danger remove-rule-btn" data-index="${idx}">${t('delete')}</button>
                </div>
                <div class="form-group" style="margin-bottom:8px;">
                    <label>${t('targetUpstreamModel')}</label>
                    <select class="rule-target" style="width:100%;">${optionsHtml}</select>
                </div>
                <div class="form-group" style="margin-bottom:8px;">
                    <label>${t('conditionType')}</label>
                    <select class="rule-condition-type" style="width:100%;">
                        <option value="token_gt" ${conditionType === 'token_gt' ? 'selected' : ''}>${t('inputTokenGt')}</option>
                        <option value="token_lt" ${conditionType === 'token_lt' ? 'selected' : ''}>${t('inputTokenLt')}</option>
                        <option value="keyword" ${conditionType === 'keyword' ? 'selected' : ''}>${t('keywordMatch')}</option>
                        <option value="has_image" ${conditionType === 'has_image' ? 'selected' : ''}>${t('hasImageOption')}</option>
                        <option value="time_range" ${conditionType === 'time_range' ? 'selected' : ''}>${t('timeRangeOption')}</option>
                    </select>
                </div>
                <div class="rule-token-gt-wrap form-group" style="margin-bottom:8px;">
                    <label>${t('inputTokenGtLabel')}</label>
                    <input class="rule-min" type="number" min="0" step="1" value="${minVal}" placeholder="e.g.: 1500">
                </div>
                <div class="rule-token-lt-wrap form-group" style="margin-bottom:8px;">
                    <label>${t('inputTokenLtLabel')}</label>
                    <input class="rule-max" type="number" min="0" step="1" value="${maxVal}" placeholder="e.g.: 500">
                </div>
                <div class="rule-keyword-wrap form-group" style="margin-bottom:0;">
                    <label>${t('keywordLabel')}</label>
                    <input class="rule-keywords" type="text" value="${escapeHtml(keywordsText)}" placeholder="e.g.: code review, bugfix">
                </div>
                <div class="rule-has-image-wrap form-group" style="margin-bottom:0;">
                    <label>${t('hasImageLabel')}</label>
                </div>
                <div class="rule-time-range-wrap form-group" style="margin-bottom:0;">
                    <label>${t('timeRangeLabel')}</label>
                    <div style="display:flex;gap:8px;align-items:center;">
                        <input class="rule-start-time" type="time" step="60" value="${escapeHtml(startTime)}">
                        <span>${t('to')}</span>
                        <input class="rule-end-time" type="time" step="60" value="${escapeHtml(endTime)}">
                    </div>
                </div>
            </div>
        `;
    }).join('');
}

function inferConditionType(rule) {
    if (rule.start_time && rule.end_time) return 'time_range';
    if (rule.has_image) return 'has_image';
    if (Array.isArray(rule.keywords) && rule.keywords.length > 0) return 'keyword';
    if (Number.isInteger(rule.min_input_tokens) && rule.min_input_tokens >= 0) return 'token_gt';
    if (Number.isInteger(rule.max_input_tokens) && rule.max_input_tokens >= 0) return 'token_lt';
    return 'token_gt';
}

function formatRuleTimeValue(value) {
    const text = String(value || '').trim();
    return /^\d{2}:\d{2}$/.test(text) ? text : '';
}

function isValidRuleTimeValue(value) {
    return /^([01]\d|2[0-3]):([0-5]\d)$/.test(String(value || '').trim());
}

function syncConditionalRuleRowDisplay(rowEl) {
    if (!rowEl) return;
    const type = rowEl.querySelector('.rule-condition-type')?.value || 'token_gt';
    const gtWrap = rowEl.querySelector('.rule-token-gt-wrap');
    const ltWrap = rowEl.querySelector('.rule-token-lt-wrap');
    const kwWrap = rowEl.querySelector('.rule-keyword-wrap');
    const hasImageWrap = rowEl.querySelector('.rule-has-image-wrap');
    const timeWrap = rowEl.querySelector('.rule-time-range-wrap');
    if (gtWrap) gtWrap.style.display = type === 'token_gt' ? 'block' : 'none';
    if (ltWrap) ltWrap.style.display = type === 'token_lt' ? 'block' : 'none';
    if (kwWrap) kwWrap.style.display = type === 'keyword' ? 'block' : 'none';
    if (hasImageWrap) hasImageWrap.style.display = type === 'has_image' ? 'block' : 'none';
    if (timeWrap) timeWrap.style.display = type === 'time_range' ? 'block' : 'none';
}

async function editConditionalAlias(encodedAlias = '') {
    const alias = decodeURIComponent(encodedAlias || '');
    const existing = conditionalAliasList.find(item => item.alias === alias);
    const modelOptions = collectAllModelOptions();
    const first = modelOptions[0] || null;
    if (!first && !existing) {
        showToast(t('configureUpstreamFirst'), 'error');
        return;
    }

    const defaultRule = first ? {
        upstream_id: first.upstream_id,
        model_name: first.model_name,
        condition_type: 'token_gt',
        min_input_tokens: null,
        max_input_tokens: null,
        keywords: [],
        has_image: false,
        start_time: null,
        end_time: null
    } : null;

    const rulesState = (existing?.rules || []).map((rule) => ({
        upstream_id: rule.upstream_id,
        model_name: rule.model_name,
        condition_type: inferConditionType(rule),
        min_input_tokens: Number.isFinite(rule.min_input_tokens) ? Number(rule.min_input_tokens) : null,
        max_input_tokens: Number.isFinite(rule.max_input_tokens) ? Number(rule.max_input_tokens) : null,
        keywords: Array.isArray(rule.keywords) ? rule.keywords : [],
        has_image: !!rule.has_image,
        start_time: formatRuleTimeValue(rule.start_time),
        end_time: formatRuleTimeValue(rule.end_time)
    }));
    if (rulesState.length === 0 && defaultRule) {
        rulesState.push(defaultRule);
    }

    const fallbackState = existing?.fallback || (first ? {
        upstream_id: first.upstream_id,
        model_name: first.model_name
    } : { upstream_id: '', model_name: '' });
    const visibilityAllUsers = existing?.all_users_visible ?? true;
    const visibilityUserIds = Array.isArray(existing?.user_ids) ? existing.user_ids : [];

    showModal(existing ? t('editConditionalAlias', alias) : t('newConditionalAlias'), `
        <form id="conditional-alias-form">
            <div class="form-group">
                <label>${t('aliasLabel')}</label>
                <input type="text" name="alias" value="${escapeHtml(alias)}" placeholder="e.g.: smart-chat" ${existing ? 'readonly' : ''} required>
            </div>
            <div class="form-group">
                <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                    <label style="margin:0;">${t('conditionRules')}</label>
                    <button type="button" id="add-conditional-rule-btn" class="btn btn-sm btn-primary">${t('addCondition')}</button>
                </div>
                <div id="conditional-rules-container"></div>
            </div>
            <div class="form-group">
                <label>${t('conditionalFallbackUpstreamModel')}</label>
                <select id="conditional-fallback-select" style="width:100%;">
                    ${buildRouteTargetOptions(modelOptions, fallbackState.upstream_id, fallbackState.model_name)}
                </select>
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('saveConditionalAlias')}</button>
        </form>
    `);

    const formEl = document.getElementById('conditional-alias-form');
    const rulesContainerEl = document.getElementById('conditional-rules-container');
    const addRuleBtnEl = document.getElementById('add-conditional-rule-btn');

    function rerenderRules() {
        rulesContainerEl.innerHTML = renderConditionalAliasRuleRows(rulesState, modelOptions) || `<div style="padding:8px;color:#6b7280;">${t('noConditionsFallbackOnly')}</div>`;
        rulesContainerEl.querySelectorAll('.conditional-rule-row').forEach((rowEl) => {
            syncConditionalRuleRowDisplay(rowEl);
        });
    }

    addRuleBtnEl.addEventListener('click', () => {
        const template = defaultRule || {
            upstream_id: fallbackState.upstream_id,
            model_name: fallbackState.model_name,
            condition_type: 'token_gt',
            min_input_tokens: null,
            max_input_tokens: null,
            keywords: [],
            has_image: false,
            start_time: null,
            end_time: null
        };
        rulesState.push({
            upstream_id: template.upstream_id,
            model_name: template.model_name,
            condition_type: 'token_gt',
            min_input_tokens: null,
            max_input_tokens: null,
            keywords: [],
            has_image: false,
            start_time: null,
            end_time: null
        });
        rerenderRules();
    });

    rulesContainerEl.addEventListener('change', (event) => {
        const typeSelect = event.target.closest('.rule-condition-type');
        if (!typeSelect) return;
        const rowEl = typeSelect.closest('.conditional-rule-row');
        syncConditionalRuleRowDisplay(rowEl);
    });

    rulesContainerEl.addEventListener('click', (event) => {
        const btn = event.target.closest('.remove-rule-btn');
        if (!btn) return;
        const idx = Number(btn.dataset.index);
        if (!Number.isInteger(idx) || idx < 0 || idx >= rulesState.length) return;
        rulesState.splice(idx, 1);
        rerenderRules();
    });

    rerenderRules();

    formEl.addEventListener('submit', async (e) => {
        e.preventDefault();
        const form = new FormData(formEl);
        const formAlias = (form.get('alias') || '').toString().trim();

        if (!formAlias) {
            showToast(t('aliasCannotEmpty'), 'error');
            return;
        }

        const rules = [];
        const rowEls = Array.from(formEl.querySelectorAll('.conditional-rule-row'));
        for (let i = 0; i < rowEls.length; i += 1) {
            const row = rowEls[i];
            const target = decodeRouteValue(row.querySelector('.rule-target')?.value || '');
            if (!target.upstream_id || !target.model_name) {
                showToast(`t('targetModelCannotEmpty', i + 1)`, 'error');
                return;
            }
            const conditionType = row.querySelector('.rule-condition-type')?.value || '';
            const minRaw = row.querySelector('.rule-min')?.value?.trim() || '';
            const maxRaw = row.querySelector('.rule-max')?.value?.trim() || '';
            const minVal = minRaw === '' ? null : Number(minRaw);
            const maxVal = maxRaw === '' ? null : Number(maxRaw);
            const keywords = (row.querySelector('.rule-keywords')?.value || '')
                .split(',')
                .map(s => s.trim())
                .filter(Boolean);
            const startTime = (row.querySelector('.rule-start-time')?.value || '').trim();
            const endTime = (row.querySelector('.rule-end-time')?.value || '').trim();

            if (conditionType === 'token_gt') {
                if (minVal === null || !Number.isInteger(minVal) || minVal < 0) {
                    showToast(t('tokenGtMustBeNonNegInt', i + 1), 'error');
                    return;
                }
                rules.push({
                    upstream_id: target.upstream_id,
                    model_name: target.model_name,
                    min_input_tokens: minVal,
                    max_input_tokens: null,
                    keywords: [],
                    has_image: false,
                    start_time: null,
                    end_time: null
                });
                continue;
            }

            if (conditionType === 'token_lt') {
                if (maxVal === null || !Number.isInteger(maxVal) || maxVal < 0) {
                    showToast(t('tokenLtMustBeNonNegInt', i + 1), 'error');
                    return;
                }
                rules.push({
                    upstream_id: target.upstream_id,
                    model_name: target.model_name,
                    min_input_tokens: null,
                    max_input_tokens: maxVal,
                    keywords: [],
                    has_image: false,
                    start_time: null,
                    end_time: null
                });
                continue;
            }

            if (conditionType === 'keyword') {
                if (keywords.length === 0) {
                    showToast(`t('atLeastOneKeyword', i + 1)`, 'error');
                    return;
                }
                rules.push({
                    upstream_id: target.upstream_id,
                    model_name: target.model_name,
                    min_input_tokens: null,
                    max_input_tokens: null,
                    keywords,
                    has_image: false,
                    start_time: null,
                    end_time: null
                });
                continue;
            }

            if (conditionType === 'has_image') {
                rules.push({
                    upstream_id: target.upstream_id,
                    model_name: target.model_name,
                    min_input_tokens: null,
                    max_input_tokens: null,
                    keywords: [],
                    has_image: true,
                    start_time: null,
                    end_time: null
                });
                continue;
            }

            if (conditionType === 'time_range') {
                if (!isValidRuleTimeValue(startTime) || !isValidRuleTimeValue(endTime)) {
                    showToast(`t('timeRangeBothRequired', i + 1)`, 'error');
                    return;
                }
                if (startTime >= endTime) {
                    showToast(`t('timeRangeInvalid', i + 1)`, 'error');
                    return;
                }
                rules.push({
                    upstream_id: target.upstream_id,
                    model_name: target.model_name,
                    min_input_tokens: null,
                    max_input_tokens: null,
                    keywords: [],
                    has_image: false,
                    start_time: startTime,
                    end_time: endTime
                });
                continue;
            }

            showToast(t('invalidConditionType', i + 1), 'error');
                return;
        }

        const fallback = decodeRouteValue(document.getElementById('conditional-fallback-select')?.value || '');
        if (!fallback.upstream_id || !fallback.model_name) {
            showToast(t('fallbackCannotEmpty'), 'error');
            return;
        }
        try {
            await api(`/models/conditional-aliases/${encodeURIComponent(formAlias)}`, {
                method: 'PUT',
                body: JSON.stringify({
                    rules,
                    fallback,
                    all_users_visible: visibilityAllUsers,
                    user_ids: visibilityUserIds
                })
            });
            hideModal();
            showToast(t('conditionalAliasSaved'), 'success');
            loadConditionalAliasesPage();
        } catch (error) {
            showToast(t('saveConditionalAliasFailed') + error.message, 'error');
        }
    });
}
function openConditionalAliasEditor(encodedAlias = '') {
    editConditionalAlias(encodedAlias).catch(error => {
        showToast(t('openEditorFailed') + error.message, 'error');
    });
}

function editConditionalAliasVisibility(encodedAlias = '') {
    const alias = decodeURIComponent(encodedAlias || '');
    const existing = conditionalAliasList.find(item => item.alias === alias);
    if (!existing) {
        showToast(t('aliasNotExist'), 'error');
        return;
    }
    const allVisible = existing?.all_users_visible ?? true;
    const selectedUserIds = Array.isArray(existing?.user_ids) ? existing.user_ids : [];
    const usersHtml = allUsersList.map(user => `
        <label style="display:block; margin:5px 0;">
            <input type="checkbox" value="${user.id}" ${selectedUserIds.includes(user.id) ? 'checked' : ''}>
            ${escapeHtml(user.username)} (${escapeHtml(user.email)})
        </label>
    `).join('');

    showModal(`t('conditionalAliasVisibility', ${alias}`, `
        <form id="conditional-alias-visibility-form">
            <div class="form-group">
                <label>
                    <input type="checkbox" name="conditional_all_users_visible" ${allVisible ? 'checked' : ''}>
                    ${t('allUsersVisibleCheckbox')}
                </label>
            </div>
            <div class="form-group" id="conditional-specific-users" style="${allVisible ? 'display:none' : ''}">
                <label>${t('specifiedVisibleUsers')}</label>
                ${usersHtml || t('noUsers')}
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('saveVisibility')}</button>
        </form>
    `);

    const formEl = document.getElementById('conditional-alias-visibility-form');
    const allUsersVisibleEl = formEl.querySelector('input[name="conditional_all_users_visible"]');
    const specificUsersEl = document.getElementById('conditional-specific-users');
    allUsersVisibleEl?.addEventListener('change', function() {
        if (specificUsersEl) specificUsersEl.style.display = this.checked ? 'none' : 'block';
    });

    formEl.addEventListener('submit', async (e) => {
        e.preventDefault();
        const form = new FormData(formEl);
        const allUsersVisible = form.get('conditional_all_users_visible') === 'on';
        const userIds = allUsersVisible
            ? []
            : Array.from(formEl.querySelectorAll('#conditional-specific-users input:checked')).map(cb => cb.value);
        if (!allUsersVisible && userIds.length === 0) {
            showToast(t('selectAtLeastOneUser'), 'error');
            return;
        }
        try {
            await api(`/models/conditional-aliases/${encodeURIComponent(alias)}/visibility`, {
                method: 'PUT',
                body: JSON.stringify({
                    all_users_visible: allUsersVisible || userIds.length === 0,
                    user_ids: userIds
                })
            });
            hideModal();
            showToast(t('visibilitySaved'), 'success');
            loadConditionalAliasesPage();
        } catch (error) {
            showToast(t('saveVisibilityFailed') + error.message, 'error');
        }
    });
}

async function deleteConditionalAlias(encodedAlias = '') {
    const alias = decodeURIComponent(encodedAlias || '');
    if (!alias) return;
    if (!confirm(t('confirmDeleteAlias', alias))) return;
    try {
        await api(`/models/conditional-aliases/${encodeURIComponent(alias)}`, {
            method: 'DELETE'
        });
        showToast(t('aliasDeleted'), 'success');
        loadConditionalAliasesPage();
    } catch (error) {
        showToast(t('deleteAliasFailed') + error.message, 'error');
    }
}
window.editConditionalAlias = editConditionalAlias;
window.openConditionalAliasEditor = openConditionalAliasEditor;

async function saveModelAlias(upstreamId, encodedModelName, inputId) {
    try {
        const modelName = decodeURIComponent(encodedModelName);
        const aliasInput = document.getElementById(inputId);
        const modelAliases = (aliasInput?.value || '')
            .toString()
            .split(',')
            .map(s => s.trim())
            .filter(Boolean);
        const visibilityRes = await api('/models').catch(() => []);
        const vis = visibilityRes.find(v => v.upstream_id === upstreamId && v.model_name === modelName);
        const allUsersVisible = vis?.all_users_visible ?? true;
        const allowedUsers = vis?.allowed_users || [];
        const modelHeaders = vis?.model_headers && typeof vis.model_headers === 'object' ? vis.model_headers : {};
        const retrySettings = getModelRetrySettings(vis);
        await api(`/models/${upstreamId}/${encodedModelName}`, {
            method: 'PUT',
            body: JSON.stringify({
                all_users_visible: allUsersVisible || allowedUsers.length === 0,
                user_ids: allUsersVisible ? [] : allowedUsers,
                model_aliases: modelAliases,
                model_headers: modelHeaders,
                retry_count: retrySettings.retry_count,
                retry_interval_seconds: retrySettings.retry_interval_seconds,
                retry_backoff_strategy: retrySettings.retry_backoff_strategy,
                retry_max_interval_seconds: retrySettings.retry_max_interval_seconds,
                retry_failure_strategy: retrySettings.retry_failure_strategy,
                retry_fallback_upstream_id: retrySettings.retry_fallback_upstream_id,
                retry_fallback_model_name: retrySettings.retry_fallback_model_name
            })
        });
        showToast(t('aliasSaved'), 'success');
        loadModels();
    } catch (error) {
        showToast(t('aliasSaveFailed') + error.message, 'error');
    }
}

async function addManualModel() {
    try {
        const upstreamId = (document.getElementById('manual-model-upstream-select')?.value || '').trim();
        const modelName = (document.getElementById('manual-model-name-input')?.value || '').trim();
        if (!upstreamId) {
            showToast(t('selectUpstream'), 'error');
            return;
        }
        if (!modelName) {
            showToast(t('enterModelName'), 'error');
            return;
        }
        const exists = upstreamModelsList
            .find(u => u.upstream_id === upstreamId)
            ?.models?.includes(modelName);
        if (exists) {
            showToast(t('modelExists'), 'error');
            return;
        }
        await api(`/models/${upstreamId}/${encodeURIComponent(modelName)}`, {
            method: 'PUT',
            body: JSON.stringify({
                all_users_visible: true,
                user_ids: [],
                model_aliases: [],
                model_headers: {},
                retry_count: 0,
                retry_interval_seconds: 0,
                retry_failure_strategy: 'error',
                retry_fallback_upstream_id: null,
                retry_fallback_model_name: null
            })
        });
        showToast(t('modelAdded'), 'success');
        document.getElementById('manual-model-name-input').value = '';
        loadModels();
    } catch (error) {
        showToast(t('addModelFailed') + error.message, 'error');
    }
}

async function testManualModel() {
    const upstreamId = (document.getElementById('manual-model-upstream-select')?.value || '').trim();
    const modelName = (document.getElementById('manual-model-name-input')?.value || '').trim();
    if (!upstreamId) {
        showToast(t('selectUpstream'), 'error');
        return;
    }
    if (!modelName) {
        showToast(t('enterModelName'), 'error');
        return;
    }
    await testModelInModelPage(upstreamId, encodeURIComponent(modelName));
}

async function testModelInModelPage(upstreamId, encodedModelName) {
    const modelName = decodeURIComponent(encodedModelName || '');
    if (!modelName) {
        showToast(t('modelNameCannotEmpty'), 'error');
        return;
    }
    showToast(t('testingModel', modelName), 'info');
    try {
        const result = await api(`/upstreams/${upstreamId}/test-model`, {
            method: 'POST',
            body: JSON.stringify({
                model: modelName,
                prompt: t('modelTestPrompt')
            })
        });
        if (!result?.success) {
            showToast(t('testFailedMsg', result?.message || t('upstreamConnectionFailed')), 'error');
            return;
        }
        const preview = (result?.output_preview || '').toString().trim();
        if (preview) {
            showToast(t('testSuccessWithPreview', modelName, preview), 'success');
        } else {
            showToast(t('testSuccess', modelName), 'success');
        }
    } catch (error) {
        showToast(t('testFailedMsg', error.message), 'error');
    }
}

async function toggleAllUsers(upstreamId, modelName, checked) {
    try {
        const visibilityRes = await api('/models').catch(() => []);
        const vis = visibilityRes.find(v => v.upstream_id === upstreamId && v.model_name === modelName);
        const encodedModel = encodeURIComponent(modelName);
        const retrySettings = getModelRetrySettings(vis);
        await api(`/models/${upstreamId}/${encodedModel}`, {
            method: 'PUT',
            body: JSON.stringify({
                all_users_visible: checked,
                user_ids: [],
                model_aliases: Array.isArray(vis?.model_aliases)
                    ? vis.model_aliases
                    : (vis?.model_alias ? vis.model_alias.split(',').map(s => s.trim()).filter(Boolean) : []),
                model_headers: vis?.model_headers && typeof vis.model_headers === 'object' ? vis.model_headers : {},
                retry_count: retrySettings.retry_count,
                retry_interval_seconds: retrySettings.retry_interval_seconds,
                retry_failure_strategy: retrySettings.retry_failure_strategy,
                retry_fallback_upstream_id: retrySettings.retry_fallback_upstream_id,
                retry_fallback_model_name: retrySettings.retry_fallback_model_name
            })
        });
        showToast(t('settingsUpdated'), 'success');
        loadModels();
    } catch (error) {
        showToast(t('settingsFailed') + error.message, 'error');
        loadModels();
    }
}

async function editModelVisibility(upstreamId, modelName) {
    const visibilityRes = await api('/models').catch(() => []);
    const vis = visibilityRes.find(v => v.upstream_id === upstreamId && v.model_name === modelName);
    const currentAllUsers = vis?.all_users_visible || false;
    const currentUsers = vis?.allowed_users || [];
    const currentAliases = Array.isArray(vis?.model_aliases)
        ? vis.model_aliases
        : (vis?.model_alias ? vis.model_alias.split(',').map(s => s.trim()).filter(Boolean) : []);
    const currentHeaders = vis?.model_headers && typeof vis.model_headers === 'object' ? vis.model_headers : {};
    const currentHeaderRows = Object.entries(currentHeaders);
    const retrySettings = getModelRetrySettings(vis);
    const retryModelOptions = collectAllModelOptions();
    const retryTargetOptions = buildRouteTargetOptions(
        retryModelOptions,
        retrySettings.retry_fallback_upstream_id,
        retrySettings.retry_fallback_model_name
    );
    const usersHtml = allUsersList.map(user => `
        <label style="display: block; margin: 5px 0;">
            <input type="checkbox" value="${user.id}" ${currentUsers.includes(user.id) ? 'checked' : ''}>
            ${user.username} (${user.email})
        </label>
    `).join('');
    
    showModal(`${t('modelConfig')}: ${modelName}`, `
        <form id="model-visibility-form">
            <div class="config-section">
                <div class="config-section-title">${t('basicInfo')}</div>
                <div class="form-group">
                    <label>${t('modelAliasLabel')}</label>
                    <input type="text" name="model_aliases" value="${currentAliases.join(', ')}" placeholder="e.g.: gpt-4o-mini-cn, qwen-27b-proxy">
                </div>
                <div class="form-group">
                    <label>${t('modelHeaderLabel')}</label>
                    <div>
                        <table id="model-headers-table" style="width:100%; border-collapse:collapse;">
                            <thead>
                                <tr>
                                    <th style="text-align:left; padding:6px 4px;">Header Key</th>
                                    <th style="text-align:left; padding:6px 4px;">Header Value</th>
                                    <th style="width:80px;"></th>
                                </tr>
                            </thead>
                            <tbody></tbody>
                        </table>
                        <button type="button" class="btn btn-sm btn-secondary" id="add-model-header-row" style="margin-top:8px;">${t('addHeader')}</button>
                    </div>
                </div>
            </div>

            <div class="config-section">
                <div class="config-section-title">${t('retrySettings')}</div>
                <div class="config-grid">
                    <div class="form-group">
                        <label>${t('retryCount')}</label>
                        <input type="number" name="retry_count" min="0" step="1" value="${retrySettings.retry_count}" placeholder="${t('zeroNoRetry')}">
                    </div>
                    <div class="form-group">
                        <label>${t('retryInterval')}</label>
                        <input type="number" name="retry_interval_seconds" min="0" step="1" value="${retrySettings.retry_interval_seconds}" placeholder="${t('retryWaitMs')}">
                    </div>
                    <div class="form-group">
                        <label>${t('backoffStrategy')}</label>
                        <select name="retry_backoff_strategy">
                            <option value="fixed" ${retrySettings.retry_backoff_strategy === 'fixed' ? 'selected' : ''}>${t('fixedInterval')}</option>
                            <option value="exponential" ${retrySettings.retry_backoff_strategy === 'exponential' ? 'selected' : ''}>${t('exponentialBackoff')}</option>
                            <option value="exponential_jitter" ${retrySettings.retry_backoff_strategy === 'exponential_jitter' ? 'selected' : ''}>${t('exponentialJitter')}</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label>${t('maxBackoffInterval')}</label>
                        <input type="number" name="retry_max_interval_seconds" min="0" step="1" value="${retrySettings.retry_max_interval_seconds}" placeholder="${t('zeroNoLimit')}">
                    </div>
                </div>
                <div class="form-group">
                    <label>${t('retryFailureAction')}</label>
                    <select name="retry_failure_strategy" id="retry-failure-strategy-select">
                        <option value="error" ${retrySettings.retry_failure_strategy === 'error' ? 'selected' : ''}>${t('returnError')}</option>
                        <option value="route" ${retrySettings.retry_failure_strategy === 'route' ? 'selected' : ''}>${t('routeToOther')}</option>
                    </select>
                </div>
                <div class="form-group" id="retry-fallback-target-group" style="${retrySettings.retry_failure_strategy === 'route' ? '' : 'display:none'}">
                    <label>${t('retryRouteTarget')}</label>
                    <select id="retry-fallback-target-select" style="width:100%;">${retryTargetOptions}</select>
                </div>
            </div>

            <div class="config-section">
                <div class="config-section-title">${t('visibility')}</div>
                <div class="form-group">
                    <label>
                        <input type="checkbox" name="all_users_visible" ${currentAllUsers ? 'checked' : ''}>
                        ${t('allUsersVisibleLabel')}
                    </label>
                </div>
                <div class="form-group" id="specific-users" style="${currentAllUsers ? 'display:none' : ''}">
                    <label>${t('specifiedUsersLabel')}</label>
                    ${usersHtml || t('noUsers')}
                </div>
            </div>

            <button type="submit" class="btn btn-primary btn-block">${t('save')}</button>
        </form>
    `, { wide: true });
    
    document.querySelector('input[name="all_users_visible"]').addEventListener('change', function() {
        document.getElementById('specific-users').style.display = this.checked ? 'none' : 'block';
    });

    const retryFailureStrategyEl = document.getElementById('retry-failure-strategy-select');
    const retryFallbackTargetGroupEl = document.getElementById('retry-fallback-target-group');
    retryFailureStrategyEl?.addEventListener('change', function() {
        if (retryFallbackTargetGroupEl) {
            retryFallbackTargetGroupEl.style.display = this.value === 'route' ? 'block' : 'none';
        }
    });

    const headersTbody = document.querySelector('#model-headers-table tbody');
    const appendHeaderRow = (key = '', value = '') => {
        const tr = document.createElement('tr');
        tr.innerHTML = `
            <td style="padding:4px;">
                <input type="text" class="model-header-key" value="${String(key).replace(/"/g, '&quot;')}" placeholder="e.g.: x-env">
            </td>
            <td style="padding:4px;">
                <input type="text" class="model-header-value" value="${String(value).replace(/"/g, '&quot;')}" placeholder="e.g.: prod">
            </td>
            <td style="padding:4px; text-align:right;">
                <button type="button" class="btn btn-sm btn-danger remove-model-header-row">${t('delete')}</button>
            </td>
        `;
        headersTbody.appendChild(tr);
    };
    if (currentHeaderRows.length > 0) {
        currentHeaderRows.forEach(([k, v]) => appendHeaderRow(k, v));
    } else {
        appendHeaderRow('', '');
    }
    document.getElementById('add-model-header-row').addEventListener('click', () => appendHeaderRow('', ''));
    headersTbody.addEventListener('click', (e) => {
        const btn = e.target.closest('.remove-model-header-row');
        if (!btn) return;
        const rows = headersTbody.querySelectorAll('tr');
        if (rows.length <= 1) {
            const keyInput = rows[0].querySelector('.model-header-key');
            const valueInput = rows[0].querySelector('.model-header-value');
            keyInput.value = '';
            valueInput.value = '';
            return;
        }
        btn.closest('tr')?.remove();
    });
    
    document.getElementById('model-visibility-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const form = new FormData(e.target);
        const allUsersVisible = form.get('all_users_visible') === 'on';
        const modelAliases = (form.get('model_aliases') || '')
            .toString()
            .split(',')
            .map(s => s.trim())
            .filter(Boolean);
        const modelHeaders = {};
        const seenHeaderKeys = new Set();
        const headerRows = Array.from(document.querySelectorAll('#model-headers-table tbody tr'));
        for (const row of headerRows) {
            const key = (row.querySelector('.model-header-key')?.value || '').trim();
            const value = (row.querySelector('.model-header-value')?.value || '').trim();
            if (!key && !value) {
                continue;
            }
            if (!key) {
                showToast(t('headerKeyCannotEmpty'), 'error');
                return;
            }
            const keyLower = key.toLowerCase();
            if (seenHeaderKeys.has(keyLower)) {
                showToast(t('headerKeyDuplicate', key), 'error');
                return;
            }
            seenHeaderKeys.add(keyLower);
            modelHeaders[key] = value;
        }
        const retryCount = Number((form.get('retry_count') || '0').toString());
        const retryIntervalMillis = Number((form.get('retry_interval_seconds') || '0').toString());
        const retryBackoffStrategy = (form.get('retry_backoff_strategy') || 'fixed').toString();
        const retryMaxIntervalMillis = Number((form.get('retry_max_interval_seconds') || '0').toString());
        if (!Number.isInteger(retryCount) || retryCount < 0) {
            showToast(t('retryCountMustBeNonNegInt'), 'error');
            return;
        }
        if (!Number.isInteger(retryIntervalMillis) || retryIntervalMillis < 0) {
            showToast(t('retryIntervalMustBeNonNegInt'), 'error');
            return;
        }
        if (!['fixed', 'exponential', 'exponential_jitter'].includes(retryBackoffStrategy)) {
            showToast(t('invalidBackoffStrategy'), 'error');
            return;
        }
        if (!Number.isInteger(retryMaxIntervalMillis) || retryMaxIntervalMillis < 0) {
            showToast(t('maxBackoffMustBeNonNegInt'), 'error');
            return;
        }
        const retryFailureStrategy = (form.get('retry_failure_strategy') || 'error').toString();
        const retryFallback = decodeRouteValue(document.getElementById('retry-fallback-target-select')?.value || '');
        if (retryFailureStrategy === 'route' && (!retryFallback.upstream_id || !retryFallback.model_name)) {
            showToast(t('selectRetryFailureTarget'), 'error');
            return;
        }
        const userIds = Array.from(document.querySelectorAll('#specific-users input:checked')).map(cb => cb.value);
        const encodedModel = encodeURIComponent(modelName);
        
        try {
            await api(`/models/${upstreamId}/${encodedModel}`, {
                method: 'PUT',
                body: JSON.stringify({
                    all_users_visible: allUsersVisible || userIds.length === 0,
                    user_ids: userIds,
                    model_aliases: modelAliases,
                    model_headers: modelHeaders,
                    retry_count: retryCount,
                    retry_interval_seconds: retryIntervalMillis,
                    retry_backoff_strategy: retryBackoffStrategy,
                    retry_max_interval_seconds: retryMaxIntervalMillis,
                    retry_failure_strategy: retryFailureStrategy,
                    retry_fallback_upstream_id: retryFailureStrategy === 'route' ? retryFallback.upstream_id : null,
                    retry_fallback_model_name: retryFailureStrategy === 'route' ? retryFallback.model_name : null
                })
            });
            hideModal();
            showToast(t('settingsSaved'), 'success');
            loadModels();
        } catch (error) {
            showToast(t('saveFailed') + error.message, 'error');
        }
    });
}

async function editUpstream(id) {
    try {
        const upstream = await api(`/upstreams/${id}`);
        const currentHeaderRows = Object.entries(upstream.custom_headers || {});
        
        showModal(t('editUpstream'), `
            <form id="edit-upstream-form">
                <div class="form-group">
                    <label>${t('name')}</label>
                    <input type="text" name="name" value="${upstream.name}" required>
                </div>
                <div class="form-group">
                    <label>${t('apiType')}</label>
                    <select name="api_type" required>
                        <option value="openai" ${upstream.api_type === 'openai' ? 'selected' : ''}>OpenAI API</option>
                        <option value="anthropic" ${upstream.api_type === 'anthropic' ? 'selected' : ''}>Anthropic API</option>
                        <option value="ollama" ${upstream.api_type === 'ollama' ? 'selected' : ''}>Ollama API</option>
                    </select>
                </div>
                <div class="form-group">
                    <label>Base URL</label>
                    <input type="url" name="base_url" value="${upstream.base_url}" required>
                </div>
                <div class="form-group">
                    <label>${t('apiKeyOptional')}</label>
                    <div style="display:flex; gap:8px; align-items:center;">
                        <input type="password" name="api_key" placeholder="${t('apiKeyPlaceholder')}" style="flex:1;">
                        ${upstream.api_key_masked ? `<button type="button" class="btn btn-sm btn-secondary" onclick="copyUpstreamApiKey('${upstream.id}')">${t('copyCurrentKey')}</button>` : ''}
                    </div>
                    ${upstream.api_key_masked ? `<small style="color:#94a3b8; margin-top:4px; display:block;">${t('currentKeyMasked', escapeHtml(upstream.api_key_masked))}</small>` : ''}
                </div>
                <div class="form-group">
                    <label>${t('dailyMaxRequests')}</label>
                    <input type="number" name="daily_request_limit" value="${upstream.daily_request_limit || 2000}">
                </div>
                <div class="form-group">
                    <label>${t('monthlyMaxRequests')}</label>
                    <input type="number" name="monthly_request_limit" value="${upstream.monthly_request_limit || 50000}">
                </div>
                <div class="form-group">
                    <label>${t('customHeaders')}</label>
                    <div>
                        <table id="upstream-headers-table" style="width:100%; border-collapse:collapse;">
                            <thead>
                                <tr>
                                    <th style="text-align:left; padding:6px 4px;">Header Key</th>
                                    <th style="text-align:left; padding:6px 4px;">Header Value</th>
                                    <th style="width:80px;"></th>
                                </tr>
                            </thead>
                            <tbody></tbody>
                        </table>
                        <button type="button" class="btn btn-sm btn-secondary" id="add-upstream-header-row" style="margin-top:8px;">${t('addHeader')}</button>
                    </div>
                </div>
                <div class="form-actions">
                    <button type="button" class="btn btn-secondary" onclick="testUpstreamConnection('${id}')">${t('testConnection')}</button>
                    <button type="submit" class="btn btn-primary">${t('save')}</button>
                </div>
            </form>
            <div id="test-result" class="test-result hidden"></div>
        `);

        const headersTbody = document.querySelector('#upstream-headers-table tbody');
        const appendHeaderRow = (key = '', value = '') => {
            const tr = document.createElement('tr');
            tr.innerHTML = `
                <td style="padding:4px;">
                    <input type="text" class="upstream-header-key" value="${String(key).replace(/"/g, '&quot;')}" placeholder="e.g.: X-Org-Id">
                </td>
                <td style="padding:4px;">
                    <input type="text" class="upstream-header-value" value="${String(value).replace(/"/g, '&quot;')}" placeholder="e.g.: tenant-a">
                </td>
                <td style="padding:4px; text-align:right;">
                    <button type="button" class="btn btn-sm btn-danger remove-upstream-header-row">${t('delete')}</button>
                </td>
            `;
            headersTbody.appendChild(tr);
        };
        if (currentHeaderRows.length > 0) {
            currentHeaderRows.forEach(([k, v]) => appendHeaderRow(k, v));
        } else {
            appendHeaderRow('', '');
        }
        document.getElementById('add-upstream-header-row').addEventListener('click', () => appendHeaderRow('', ''));
        headersTbody.addEventListener('click', (e) => {
            const btn = e.target.closest('.remove-upstream-header-row');
            if (!btn) return;
            const rows = headersTbody.querySelectorAll('tr');
            if (rows.length <= 1) {
                const keyInput = rows[0].querySelector('.upstream-header-key');
                const valueInput = rows[0].querySelector('.upstream-header-value');
                keyInput.value = '';
                valueInput.value = '';
                return;
            }
            btn.closest('tr')?.remove();
        });

        document.getElementById('edit-upstream-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const formData = new FormData(e.target);
            
            const body = {
                name: formData.get('name'),
                provider: formData.get('api_type'),
                api_type: formData.get('api_type'),
                base_url: formData.get('base_url'),
                daily_request_limit: parseInt(formData.get('daily_request_limit')),
                monthly_request_limit: parseInt(formData.get('monthly_request_limit'))
            };

            const customHeaders = {};
            const seenHeaderKeys = new Set();
            const headerRows = Array.from(document.querySelectorAll('#upstream-headers-table tbody tr'));
            for (const row of headerRows) {
                const key = (row.querySelector('.upstream-header-key')?.value || '').trim();
                const value = (row.querySelector('.upstream-header-value')?.value || '').trim();
                if (!key && !value) {
                    continue;
                }
                if (!key) {
                    showToast(t('headerKeyCannotEmpty'), 'error');
                    return;
                }
                const keyLower = key.toLowerCase();
                if (seenHeaderKeys.has(keyLower)) {
                    showToast(t('headerKeyDuplicate', key), 'error');
                    return;
                }
                seenHeaderKeys.add(keyLower);
                customHeaders[key] = value;
            }
            body.custom_headers = customHeaders;
            
            const apiKey = formData.get('api_key');
            if (apiKey && apiKey.trim()) {
                body.api_key = apiKey;
            }
            
            try {
                await api(`/upstreams/${id}`, {
                    method: 'PUT',
                    body: JSON.stringify(body)
                });

                hideModal();
                showToast(t('upstreamUpdated'), 'success');
                loadUpstreams();
            } catch (error) {
                showToast(error.message, 'error');
            }
        });
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function testUpstreamConnection(id) {
    const resultDiv = document.getElementById('test-result');
    resultDiv.classList.remove('hidden');
    resultDiv.innerHTML = `<p>${t('testingConnection')}</p>`;
    
    try {
        const result = await api(`/upstreams/${id}/test`, {
            method: 'POST'
        });
        
        if (result.success) {
            let modelsHtml = '';
            if (result.models && result.models.length > 0) {
                modelsHtml = `
                    <h4>${t('availableModels')} (${result.models.length})</h4>
                    <div class="models-list">
                        ${result.models.map(m => `<div class="model-item">${m.name}</div>`).join('')}
                    </div>
                `;
            }
            
            resultDiv.innerHTML = `
                <div class="alert alert-success">
                    <strong>✓ ${t('connectionSuccess')}</strong>
                    <p>${result.message}</p>
                </div>
                ${modelsHtml}
            `;
        } else {
            resultDiv.innerHTML = `
                <div class="alert alert-error">
                    <strong>✗ ${t('connectionFailed')}</strong>
                    <p>${result.message}</p>
                </div>
            `;
        }
    } catch (error) {
        resultDiv.innerHTML = `
            <div class="alert alert-error">
                <strong>✗ ${t('testFailed')}</strong>
                <p>${normalizeErrorMessage(error.message)}</p>
            </div>
        `;
    }
}

async function showCreateUpstreamModal() {
    showModal(t('addUpstreamTitle'), `
        <form id="create-upstream-form">
            <div class="form-group">
                <label>${t('name')}</label>
                <input type="text" name="name" required>
            </div>
            <div class="form-group">
                <label>${t('apiType')}</label>
                <select name="api_type" required>
                    <option value="openai">OpenAI API</option>
                    <option value="anthropic">Anthropic API</option>
                    <option value="ollama">Ollama API</option>
                </select>
            </div>
            <div class="form-group">
                <label>Base URL</label>
                <input type="url" name="base_url" placeholder="https://api.openai.com/v1" required>
            </div>
            <div class="form-group">
                <label>${t('apiKeyOptionalCreate')}</label>
                <input type="password" name="api_key" placeholder="${t('apiKeyEmptyHint')}">
            </div>
            <div class="form-group">
                <label>${t('dailyMaxRequests')}</label>
                <input type="number" name="daily_request_limit" value="2000">
            </div>
            <div class="form-group">
                <label>${t('monthlyMaxRequests')}</label>
                <input type="number" name="monthly_request_limit" value="50000">
            </div>
            <div class="form-group">
                <label>${t('customHeadersJson')}</label>
                <textarea name="custom_headers" rows="6" placeholder='{"X-Org-Id":"tenant-a","Authorization":"Bearer xxx"}'>{}</textarea>
            </div>
            <button type="submit" class="btn btn-primary btn-block">${t('add')}</button>
        </form>
    `);

    document.getElementById('create-upstream-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const formData = new FormData(e.target);
        
        const body = {
            name: formData.get('name'),
            provider: formData.get('api_type'),
            api_type: formData.get('api_type'),
            base_url: formData.get('base_url'),
            daily_request_limit: parseInt(formData.get('daily_request_limit')),
            monthly_request_limit: parseInt(formData.get('monthly_request_limit'))
        };

        const headersText = (formData.get('custom_headers') || '').toString().trim();
        if (headersText) {
            let parsedHeaders;
            try {
                parsedHeaders = JSON.parse(headersText);
            } catch (e) {
                showToast(t('invalidHeadersJson'), 'error');
                return;
            }
            if (typeof parsedHeaders !== 'object' || Array.isArray(parsedHeaders) || parsedHeaders === null) {
                showToast(t('invalidHeadersNotObject'), 'error');
                return;
            }
            body.custom_headers = parsedHeaders;
        } else {
            body.custom_headers = {};
        }
        
        const apiKey = formData.get('api_key');
        if (apiKey && apiKey.trim()) {
            body.api_key = apiKey;
        }
        
        try {
            await api('/upstreams', {
                method: 'POST',
                body: JSON.stringify(body)
            });

            hideModal();
            showToast(t('upstreamAdded'), 'success');
            loadUpstreams();
        } catch (error) {
            showToast(error.message, 'error');
        }
    });
}

async function deleteUpstream(id) {
    if (!confirm(t('confirmDeleteUpstream'))) return;
    
    try {
        await api(`/upstreams/${id}`, { method: 'DELETE' });
        showToast(t('upstreamDeleted'), 'success');
        loadUpstreams();
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function buildAuditQueryParams(includeFormat = false, includePage = true) {
    const startDate = document.getElementById('audit-start-date')?.value;
    const endDate = document.getElementById('audit-end-date')?.value;
    const selectedUsername = document.getElementById('audit-username-filter')?.value;
    const keyword = document.getElementById('audit-keyword')?.value?.trim();
    const minTokens = document.getElementById('audit-min-tokens')?.value;
    const maxTokens = document.getElementById('audit-max-tokens')?.value;
    const format = document.getElementById('audit-export-format')?.value;
    const params = new URLSearchParams();

    if (startDate) params.append('start_time', buildLocalDateBoundaryIso(startDate, false));
    if (endDate) params.append('end_time', buildLocalDateBoundaryIso(endDate, true));
    if (currentUser?.role === 'admin' && selectedUsername) {
        params.append('username', decodeURIComponent(selectedUsername));
    }
    if (keyword) params.append('keyword', keyword);
    if (minTokens !== '') params.append('min_tokens', minTokens);
    if (maxTokens !== '') params.append('max_tokens', maxTokens);
    if (includePage) {
        params.append('page', String(auditCurrentPage));
        params.append('page_size', String(AUDIT_PAGE_SIZE));
    }
    if (includeFormat && format) params.append('format', format);

    return params;
}

function formatDateInputValue(date) {
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
}

function formatServerDateInputValue(date) {
    const offsetMs = serverTimezoneOffsetMinutes * 60 * 1000;
    const shifted = new Date(date.getTime() + offsetMs);
    const year = shifted.getUTCFullYear();
    const month = String(shifted.getUTCMonth() + 1).padStart(2, '0');
    const day = String(shifted.getUTCDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
}

function buildServerDateBoundaryIso(dateText, isEndOfDay = false) {
    if (!dateText) return '';
    const [yearText, monthText, dayText] = dateText.split('-');
    const year = Number(yearText);
    const month = Number(monthText);
    const day = Number(dayText);
    if (!Number.isFinite(year) || !Number.isFinite(month) || !Number.isFinite(day)) {
        return '';
    }
    const hour = isEndOfDay ? 23 : 0;
    const minute = isEndOfDay ? 59 : 0;
    const second = isEndOfDay ? 59 : 0;
    const millisecond = isEndOfDay ? 999 : 0;
    const utcMillis = Date.UTC(year, month - 1, day, hour, minute, second, millisecond)
        - serverTimezoneOffsetMinutes * 60 * 1000;
    return new Date(utcMillis).toISOString();
}

function buildLocalDateBoundaryIso(dateText, isEndOfDay = false) {
    if (!dateText) return '';
    const [yearText, monthText, dayText] = dateText.split('-');
    const year = Number(yearText);
    const month = Number(monthText);
    const day = Number(dayText);
    if (!Number.isFinite(year) || !Number.isFinite(month) || !Number.isFinite(day)) {
        return '';
    }
    const date = isEndOfDay
        ? new Date(year, month - 1, day, 23, 59, 59, 999)
        : new Date(year, month - 1, day, 0, 0, 0, 0);
    return date.toISOString();
}

function ensureAuditDefaultDateRange() {
    const startInput = document.getElementById('audit-start-date');
    const endInput = document.getElementById('audit-end-date');
    if (!startInput || !endInput) return;
    if (startInput.value && endInput.value) return;

    const endDate = new Date();
    const startDate = new Date(endDate);
    startDate.setMonth(startDate.getMonth() - 1);

    if (!startInput.value) {
        startInput.value = formatDateInputValue(startDate);
    }
    if (!endInput.value) {
        endInput.value = formatDateInputValue(endDate);
    }
}

function getAuditTotalPages() {
    return Math.max(1, Math.ceil(auditTotalItems / AUDIT_PAGE_SIZE));
}

function setAuditLoading(loading, text = t('loadingAuditLogs')) {
    const loadingBox = document.getElementById('audit-loading');
    const loadingText = document.getElementById('audit-loading-text');
    const filterBtn = document.getElementById('audit-filter-btn');
    const exportBtn = document.getElementById('audit-export-btn');
    if (loadingBox) {
        loadingBox.classList.toggle('hidden', !loading);
    }
    if (loadingText) {
        loadingText.textContent = text;
    }
    if (filterBtn) filterBtn.disabled = loading;
    if (exportBtn) exportBtn.disabled = loading;
}

function goToAuditPage(page) {
    const totalPages = getAuditTotalPages();
    const nextPage = Math.min(Math.max(1, Number(page) || 1), totalPages);
    if (nextPage === auditCurrentPage) return;
    auditCurrentPage = nextPage;
    loadAuditLogs();
}

function renderAuditPagination() {
    const pageInfo = document.getElementById('audit-page-info');
    const resultSummary = document.getElementById('audit-result-summary');
    const prevBtn = document.getElementById('audit-prev-page-btn');
    const nextBtn = document.getElementById('audit-next-page-btn');
    const firstBtn = document.getElementById('audit-first-page-btn');
    const lastBtn = document.getElementById('audit-last-page-btn');
    const pageLinks = document.getElementById('audit-page-links');
    const jumpInput = document.getElementById('audit-jump-page-input');
    const totalPages = getAuditTotalPages();
    if (auditCurrentPage > totalPages) auditCurrentPage = totalPages;

    if (pageInfo) {
        pageInfo.textContent = t('pageOfTotal', auditCurrentPage, totalPages, auditTotalItems);
    }
    if (resultSummary) {
        resultSummary.textContent = t('resultSummary', auditTotalItems, totalPages, auditCurrentPage);
    }
    if (prevBtn) prevBtn.disabled = auditCurrentPage <= 1;
    if (nextBtn) nextBtn.disabled = auditCurrentPage >= totalPages;
    if (firstBtn) firstBtn.disabled = auditCurrentPage <= 1;
    if (lastBtn) lastBtn.disabled = auditCurrentPage >= totalPages;
    if (jumpInput) jumpInput.value = String(auditCurrentPage);

    if (!pageLinks) return;

    const nearRange = 2;
    const pages = [];
    pages.push(1);
    for (let page = Math.max(2, auditCurrentPage - nearRange); page <= Math.min(totalPages - 1, auditCurrentPage + nearRange); page++) {
        pages.push(page);
    }
    if (totalPages > 1) pages.push(totalPages);

    const uniquePages = [...new Set(pages)].sort((a, b) => a - b);
    let html = '';
    for (let i = 0; i < uniquePages.length; i++) {
        const page = uniquePages[i];
        const prev = uniquePages[i - 1];
        if (prev && page - prev > 1) {
            html += '<span>...</span>';
        }
        html += `<button class="audit-page-link ${page === auditCurrentPage ? 'active' : ''}" data-page="${page}">${page}</button>`;
    }
    pageLinks.innerHTML = html;
}

async function loadAuditLogs() {
    setAuditLoading(true, t('loadingAuditLogs'));
    try {
        let url = '/audit/proxy';
        const params = buildAuditQueryParams(false, true);
        if (params.toString()) url += `?${params.toString()}`;

        const data = await api(url);
        auditTotalItems = Number(data.total || 0);
        renderAuditPagination();
        
        const tbody = document.getElementById('audit-list');
        if (!data.items || data.items.length === 0) {
            tbody.innerHTML = `<tr><td colspan="7">${t('noAuditLogs')}</td></tr>`;
            return;
        }

        tbody.innerHTML = data.items.map(log => `
            <tr>
                <td>${formatServerDateTime(log.created_at)}</td>
                <td>${log.username || '-'}</td>
                <td>${formatModelAliasWithOriginal(log.model, log.model_alias, log.original_model_name, log.routed_model)}</td>
                <td>${log.status || '-'}</td>
                <td>${(log.total_tokens || 0).toLocaleString()}</td>
                <td>${log.client_ip || '-'}</td>
                <td>
                    <button class="btn btn-sm btn-outline" onclick="viewAuditDetails('${log.id}')">${t('details')}</button>
                </td>
            </tr>
        `).join('');
    } catch (error) {
        showToast(error.message, 'error');
    } finally {
        setAuditLoading(false);
    }
}

async function exportAuditDataset() {
    setAuditLoading(true, t('exportingDataset'));
    try {
        const token = localStorage.getItem('token');
        const params = buildAuditQueryParams(true, false);
        const url = `${API_BASE}/audit/proxy/export${params.toString() ? `?${params.toString()}` : ''}`;
        const response = await fetch(url, {
            headers: {
                'Authorization': `Bearer ${token}`
            }
        });

        if (response.status === 401) {
            handleLogout();
            throw new Error(t('sessionExpired'));
        }

        if (!response.ok) {
            const error = await response.json().catch(() => ({ error: t('exportFailed') }));
            throw new Error(error.error || t('exportFailed'));
        }

        const blob = await response.blob();
        const disposition = response.headers.get('content-disposition') || '';
        const match = disposition.match(/filename="([^"]+)"/);
        const filename = match?.[1] || 'audit-dataset-export.jsonl';
        const blobUrl = window.URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = blobUrl;
        link.download = filename;
        document.body.appendChild(link);
        link.click();
        link.remove();
        window.URL.revokeObjectURL(blobUrl);
        showToast(t('datasetExportSuccess'), 'success');
    } catch (error) {
        showToast(error.message, 'error');
    } finally {
        setAuditLoading(false);
    }
}

async function loadAuditFilters() {
    ensureAuditDefaultDateRange();
    const userFilter = document.getElementById('audit-username-filter');
    if (!userFilter) return;

    if (currentUser?.role !== 'admin') {
        userFilter.classList.add('hidden');
        userFilter.value = '';
        return;
    }

    userFilter.classList.remove('hidden');

    if (auditUsersLoaded) return;

    try {
        const data = await api('/users');
        const options = (data.items || []).map(user => `
            <option value="${encodeURIComponent(user.username)}">${escapeHtml(user.username)}${user.role === 'admin' ? t('adminTag') : ''}</option>
        `).join('');
        userFilter.innerHTML = `<option value="">${t('allUsersFilter')}</option>${options}`;
        auditUsersLoaded = true;
    } catch (error) {
        showToast(error.message, 'error');
    }
}

async function viewAuditDetails(id) {
    try {
        const log = await api(`/audit/proxy/${id}`);
        const requestBody = formatJsonBlock(log.request_body);
        const responseBody = formatJsonBlock(log.response_body);
        const messageCount = Array.isArray(log.messages) ? log.messages.length : 0;
        let messagesHtml;
        if (log.content_deleted) {
            messagesHtml = `<div class="audit-message-card" style="text-align:center;color:#999;padding:24px;">${t('contentDeleted')}</div>`;
        } else {
            messagesHtml = (log.messages || []).map(msg => {
                const messageText = formatAuditMessageText(msg);
                return `
                <div class="audit-message-card audit-role-${escapeHtml(msg.role || 'unknown')}">
                    <div class="audit-message-header">
                        <span class="audit-role-badge">${escapeHtml(msg.role || 'unknown')}</span>
                        <button class="btn btn-sm btn-outline" type="button" data-copy-text="${encodeURIComponent(messageText)}">${t('copy')}</button>
                    </div>
                    <pre class="audit-pre audit-message-pre">${escapeHtml(messageText || '')}</pre>
                </div>
            `;
            }).join('') || `<div>${t('noMessageContent')}</div>`;
        }
        const dialogSection = log.content_deleted
            ? `<div class="audit-detail-section"><div class="audit-section-bar"><div class="audit-section-title">${t('dialogContent')}</div></div>${messagesHtml}</div>`
            : `<div class="audit-detail-section"><div class="audit-section-bar"><div class="audit-section-title">${t('dialogContent')}</div><div class="audit-section-meta">${t('messageCount', messageCount)}</div></div><div class="audit-messages-list">${messagesHtml}</div></div>`;
        const requestSection = log.content_deleted ? '' : `<details class="audit-detail-section" open>
                    <summary class="audit-collapsible-title">${t('requestBody')}</summary>
                    <div class="audit-json-toolbar">
                        <button id="copy-audit-request-body-btn" class="btn btn-sm btn-outline" type="button">${t('copyRequestBody')}</button>
                    </div>
                    <pre class="audit-pre">${escapeHtml(requestBody)}</pre>
                </details>`;
        const responseSection = log.content_deleted ? '' : `<details class="audit-detail-section">
                    <summary class="audit-collapsible-title">${t('responseBody')}</summary>
                    <div class="audit-json-toolbar">
                        <button id="copy-audit-response-body-btn" class="btn btn-sm btn-outline" type="button">${t('copyResponseBody')}</button>
                    </div>
                    <pre class="audit-pre">${escapeHtml(responseBody)}</pre>
                </details>`;
        showModal(t('auditDetailTitle'), `
            <div class="audit-detail-layout">
                <div class="audit-summary-grid">
                    <div class="audit-summary-card"><div class="audit-summary-label">${t('time')}</div><div class="audit-summary-value">${formatServerDateTime(log.created_at)}</div></div>
                    <div class="audit-summary-card"><div class="audit-summary-label">${t('user')}</div><div class="audit-summary-value">${escapeHtml(log.username || '-')}</div></div>
                    <div class="audit-summary-card"><div class="audit-summary-label">${t('status')}</div><div class="audit-summary-value">${escapeHtml(log.status || '-')}</div></div>
                    <div class="audit-summary-card"><div class="audit-summary-label">Token</div><div class="audit-summary-value">${Number(log.total_tokens || 0).toLocaleString()}</div></div>
                    <div class="audit-summary-card"><div class="audit-summary-label">IP</div><div class="audit-summary-value">${escapeHtml(log.client_ip || '-')}</div></div>
                    <div class="audit-summary-card"><div class="audit-summary-label">${t('conversationId')}</div><div class="audit-summary-value audit-mono-text">${escapeHtml(log.conversation_id || '-')}</div></div>
                </div>
                <div class="audit-detail-section">
                    <div class="audit-section-title">${t('modelInfo')}</div>
                    <div class="audit-model-block">${formatModelAliasWithOriginal(log.model, log.model_alias, log.original_model_name, log.routed_model)}</div>
                </div>
                ${dialogSection}
                ${requestSection}
                ${responseSection}
            </div>
        `, { wide: true });
        if (!log.content_deleted) {
            const requestBtn = document.getElementById('copy-audit-request-body-btn');
            if (requestBtn) {
                requestBtn.onclick = () => copyToClipboard(requestBody);
            }
            const responseBtn = document.getElementById('copy-audit-response-body-btn');
            if (responseBtn) {
                responseBtn.onclick = () => copyToClipboard(responseBody);
            }
        }
        document.querySelectorAll('[data-copy-text]').forEach((btn) => {
            btn.onclick = () => copyEncodedText(btn.dataset.copyText || '');
        });
    } catch (error) {
        showToast(error.message, 'error');
    }
}

function formatJsonBlock(value) {
    if (value == null || value === '') return '';
    if (typeof value !== 'string') {
        try {
            return JSON.stringify(value, null, 2);
        } catch (_) {
            return String(value);
        }
    }
    try {
        return JSON.stringify(JSON.parse(value), null, 2);
    } catch (_) {
        return value;
    }
}

function formatAuditMessageText(message) {
    if (!message || typeof message !== 'object') {
        return '';
    }
    const sections = [];
    if (message.content) {
        sections.push(String(message.content));
    }
    if (message.reasoning_content) {
        sections.push(String(message.reasoning_content));
    }
    if (Array.isArray(message.tool_calls) && message.tool_calls.length > 0) {
        sections.push(...message.tool_calls.map((item) => formatJsonBlock(item)));
    }
    if (message.function_call) {
        sections.push(formatJsonBlock(message.function_call));
    }
    return sections.filter(Boolean).join('\n\n');
}

function showModal(title, content, options = {}) {
    modalPreviousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    document.getElementById('modal-title').textContent = title;
    document.getElementById('modal-body').innerHTML = content;
    const modal = document.getElementById('modal');
    modal.classList.remove('hidden');
    modal.setAttribute('aria-hidden', 'false');
    document.body.classList.add('modal-open');
    const modalContent = document.querySelector('#modal .modal-content');
    if (modalContent) {
        modalContent.classList.toggle('modal-wide', !!options?.wide);
        window.setTimeout(() => modalContent.focus(), 0);
    }
}

function hideModal() {
    const modal = document.getElementById('modal');
    modal.classList.add('hidden');
    modal.setAttribute('aria-hidden', 'true');
    document.body.classList.remove('modal-open');
    const modalContent = document.querySelector('#modal .modal-content');
    if (modalContent) {
        modalContent.classList.remove('modal-wide');
    }
    if (modalPreviousFocus) {
        modalPreviousFocus.focus();
        modalPreviousFocus = null;
    }
}

function maskApiKey(key) {
    if (!key) return '';
    if (key.startsWith('sk-')) {
        const body = key.slice(3);
        if (body.length <= 8) {
            return 'sk-' + '*'.repeat(body.length - 4) + body.slice(-4);
        }
        return 'sk-' + body.slice(0, 4) + '****' + body.slice(-4);
    }
    if (key.length <= 8) {
        return '*'.repeat(key.length - 4) + key.slice(-4);
    }
    return key.slice(0, 4) + '****' + key.slice(-4);
}

function formatDisplayKey(keyPrefix, keySuffix) {
    if (!keyPrefix) return '';
    const suffix = keySuffix || '';
    if (keyPrefix.startsWith('sk-')) {
        const body = keyPrefix.slice(3);
        if (suffix) {
            return 'sk-' + body.slice(0, 4) + '****' + suffix;
        }
        if (body.length <= 4) {
            return 'sk-' + '*'.repeat(body.length) ;
        }
        return 'sk-' + body.slice(0, 4) + '****';
    }
    if (suffix) {
        return keyPrefix.slice(0, 4) + '****' + suffix;
    }
    return keyPrefix.slice(0, 4) + '****';
}

async function copyToClipboard(text) {
    const value = text == null ? '' : String(text);
    try {
        if (navigator.clipboard && window.isSecureContext) {
            await navigator.clipboard.writeText(value);
            showToast(t('copiedToClipboard'), 'success');
            return;
        }

        const textarea = document.createElement('textarea');
        textarea.value = value;
        textarea.setAttribute('readonly', '');
        textarea.style.position = 'fixed';
        textarea.style.opacity = '0';
        textarea.style.pointerEvents = 'none';
        document.body.appendChild(textarea);
        textarea.select();
        const ok = document.execCommand('copy');
        document.body.removeChild(textarea);

        if (!ok) throw new Error(t('copyFailed'));
        showToast(t('copiedToClipboard'), 'success');
    } catch {
        showToast(t('copyFailed'), 'error');
    }
}

function copyEncodedText(encodedText) {
    copyToClipboard(decodeURIComponent(encodedText || ''));
}

function copyApiKeyPrefix(prefix) {
    copyToClipboard(prefix || '');
}

function showToast(message, type = 'info') {
    const normalizedMessage = type === 'error'
        ? normalizeErrorMessage(message)
        : String(message ?? '').trim();
    if (!normalizedMessage || normalizedMessage === STALE_SESSION_MESSAGE) {
        return;
    }
    const region = document.getElementById('toast-region') || document.body;
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = normalizedMessage;
    toast.setAttribute('role', type === 'error' ? 'alert' : 'status');
    region.appendChild(toast);

    setTimeout(() => {
        toast.remove();
    }, 3000);
}

async function loadSettings() {
    // Set up password change form event listeners
    const form = document.getElementById('change-password-form');
    if (form) {
        form.onsubmit = handleChangePassword;
    }
    
    // If admin, show system settings
    const systemSettingsSection = document.getElementById('system-settings-section');
    if (currentUser?.role === 'admin') {
        if (systemSettingsSection) {
            systemSettingsSection.classList.remove('hidden');
        }
        
        // Load system settings
        try {
            const response = await fetch(`${API_BASE}/settings`, {
                headers: { 'Authorization': `Bearer ${localStorage.getItem('token')}` }
            });
            if (response.ok) {
                const data = await response.json();
                document.getElementById('system-base-url').value = data.base_url;
            }
        } catch (e) {
            console.error('Failed to load system settings:', e);
        }
        
        // Set up system settings form event listeners
        const systemForm = document.getElementById('system-settings-form');
        if (systemForm) {
            systemForm.onsubmit = handleSaveSystemSettings;
        }
    } else {
        if (systemSettingsSection) {
            systemSettingsSection.classList.add('hidden');
        }
    }
}

async function handleSaveSystemSettings(e) {
    e.preventDefault();
    
    const baseUrl = document.getElementById('system-base-url').value;
    
    try {
        const response = await fetch(`${API_BASE}/settings`, {
            method: 'PUT',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${localStorage.getItem('token')}`
            },
            body: JSON.stringify({ base_url: baseUrl })
        });
        
        if (response.ok) {
            showToast(t('systemSettingsSaved'), 'success');
            // Update locally cached base_url
            systemBaseUrl = baseUrl;
        } else {
            const error = await response.json();
            throw new Error(error.error || t('saveSystemSettingsFailed'));
        }
    } catch (e) {
        showToast(e.message, 'error');
    }
}

async function handleChangePassword(e) {
    e.preventDefault();
    
    const currentPassword = document.getElementById('current-password').value;
    const newPassword = document.getElementById('new-password').value;
    const confirmPassword = document.getElementById('confirm-password').value;
    
    // Verify new password and confirmation match
    if (newPassword !== confirmPassword) {
        showToast(t('passwordMismatch'), 'error');
        return;
    }
    
    // Verify password length
    if (newPassword.length < 6) {
        showToast(t('passwordTooShort'), 'error');
        return;
    }
    
    try {
        const response = await fetch(`${AUTH_BASE}/change-password`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${localStorage.getItem('token')}`
            },
            body: JSON.stringify({
                old_password: currentPassword,
                new_password: newPassword
            })
        });
        
        if (response.ok) {
            showToast(t('passwordChanged'), 'success');
            document.getElementById('change-password-form').reset();
        } else {
            const error = await response.json();
            showToast(error.error || t('passwordChangeFailed'), 'error');
        }
    } catch (error) {
        showToast(t('networkError'), 'error');
    }
}

document.addEventListener('DOMContentLoaded', init);
