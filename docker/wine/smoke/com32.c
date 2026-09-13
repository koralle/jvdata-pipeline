/* 32-bit COM スモークテスト。
 *
 * このイメージが (1) 32-bit の Windows 実行体を起動できて (2) COM サーバーを
 * 活性化できることを確かめる。JV-Link 自体は手作業でのインストールと利用登録が
 * 要るので、ここで見るのは Wine 側だけ。
 *
 * 32-bit 側が壊れた prefix でも Wine 自体は起動してしまい、失敗は「応答しない
 * 32-bit プロセス」として現れる。そうなる前にここで落とす。
 */
#include <stdio.h>
#include <windows.h>

/* ShellLink (00021401-0000-0000-C000-000000000046) は Wine が実装している
 * 標準の COM サーバー。JV-Link の代わりに COM 活性化の経路を確かめる。 */
static const GUID shell_link_clsid = {
    0x00021401, 0x0000, 0x0000, {0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}};
static const GUID ishell_link_w_iid = {
    0x000214F9, 0x0000, 0x0000, {0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}};

int main(void)
{
    IUnknown *unknown = NULL;
    HRESULT hr;

    printf("com32: %u-bit build\n", (unsigned)(sizeof(void *) * 8));
    if (sizeof(void *) != 4) {
        fprintf(stderr, "com32: expected a 32-bit build\n");
        return 1;
    }

    hr = CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    if (FAILED(hr)) {
        fprintf(stderr, "com32: CoInitializeEx failed: 0x%08lX\n", (unsigned long)hr);
        return 1;
    }

    hr = CoCreateInstance(&shell_link_clsid, NULL, CLSCTX_INPROC_SERVER,
                          &ishell_link_w_iid, (void **)&unknown);
    if (FAILED(hr) || unknown == NULL) {
        fprintf(stderr, "com32: CoCreateInstance failed: 0x%08lX\n", (unsigned long)hr);
        CoUninitialize();
        return 1;
    }

    unknown->lpVtbl->Release(unknown);
    CoUninitialize();

    printf("com32: OK (32-bit COM activation works)\n");
    return 0;
}
