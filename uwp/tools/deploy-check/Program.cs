using System;
using System.IO;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;

/// <summary>
/// 签名准备 —— 生成与清单 Publisher 匹配的测试证书，装进当前用户的
/// TrustedPeople，然后签名 MSIX。
///
/// 为什么不用 PowerShell：同一个任务在 PowerShell 里要写 8 行 cmdlet、
/// 读输出才知道成没成；这里跑完直接给结论。
///
/// **做不到的一步**：把证书写进 <c>LocalMachine\Root</c> 需要管理员权限。
/// 没有它，<c>Add-AppxPackage</c> 会在 0x800B0109 停下。本工具会明确说清这一点，
/// 而不是让人从错误码里猜。
/// </summary>
static class Program
{
    const string Subject = "CN=ProjectArcturius";

    static int Main(string[] args)
    {
        var sign = args.Length == 0 || args[0] != "--check-only";

        Console.WriteLine("Saiku Deck UWP · 签名准备");
        Console.WriteLine("════════════════════════════════════════");

        // ── 1. 找包 ────────────────────────────────────────────────
        var msix = FindMsix();
        if (msix == null)
        {
            Console.WriteLine("✗ 找不到 MSIX —— 先构建");
            return 1;
        }
        Console.WriteLine($"包: {Path.GetFileName(msix)}");

        // ── 2. 找或造证书 ──────────────────────────────────────────
        var cert = FindCert();
        if (cert == null)
        {
            Console.WriteLine();
            Console.WriteLine("未找到匹配的证书，新建一个…");
            cert = CreateCert();
            if (cert == null)
            {
                Console.WriteLine("✗ 建证书失败");
                return 1;
            }
            Console.WriteLine($"✓ 已建并写入 CurrentUser\\My: {cert.Thumbprint}");
        }
        else
        {
            Console.WriteLine($"✓ 已找到证书: {cert.Thumbprint}");
        }

        Console.WriteLine($"  主体: {cert.Subject}");

        // ── 3. 把证书放进 TrustedPeople（当前用户，不需管理员）────
        var inTrustedPeople = Exists(StoreName.TrustedPeople, cert.Thumbprint);
        if (!inTrustedPeople)
        {
            Console.WriteLine("  正在写入 CurrentUser\\TrustedPeople…");
            inTrustedPeople = AddTo(StoreName.TrustedPeople, cert);
        }
        Console.WriteLine(inTrustedPeople
            ? "  ✓ 已在 CurrentUser\\TrustedPeople"
            : "  ✗ 写入 TrustedPeople 失败");

        // ── 4. 检查 Root（需要管理员）──────────────────────────────
        var inRoot = Exists(StoreName.Root, cert.Thumbprint);
        Console.WriteLine(inRoot
            ? "  ✓ 已在 CurrentUser\\Root"
            : "  ✗ 不在 CurrentUser\\Root —— 写这一处需要管理员权限");

        if (!sign)
        {
            Report(cert, inRoot);
            return inRoot ? 0 : 1;
        }

        // ── 5. 签名 ────────────────────────────────────────────────
        Console.WriteLine();
        Console.WriteLine("正在签名…");
        if (Sign(msix, cert))
        {
            Console.WriteLine($"✓ 已签名: {Path.GetFileName(msix)}");
        }
        else
        {
            Console.WriteLine("✗ 签名失败");
            return 1;
        }

        // ── 6. 验证签名 ────────────────────────────────────────────
        Console.WriteLine();
        Console.WriteLine("验证签名…");
        var hasSig = HasSignatureBlock(msix);
        Console.WriteLine(hasSig
            ? "  ✓ 包内有 AppxSignature.p7x"
            : "  ✗ 包内没有签名块");

        Report(cert, inRoot);
        return inRoot && hasSig ? 0 : 1;
    }

    static void Report(X509Certificate2 cert, bool inRoot)
    {
        Console.WriteLine();
        Console.WriteLine("════════════════════════════════════════");
        Console.WriteLine("结论");
        Console.WriteLine();

        if (inRoot)
        {
            Console.WriteLine("签名：**完成** —— 证书可信，包已签。");
            Console.WriteLine("下一步：Add-AppxPackage -Path <msix>");
        }
        else
        {
            Console.WriteLine("签名：**做了一半** —— 包已签，证书在 TrustedPeople，");
            Console.WriteLine("但**没在受信任的根**，安装仍会被 0x800B0109 挡住。");
            Console.WriteLine();
            Console.WriteLine("缺的这一步需要管理员权限，本会话做不到：");
            Console.WriteLine();
            Console.WriteLine("    # 以管理员身份开一个 PowerShell，跑：");
            Console.WriteLine($"    Export-Certificate -Cert (Get-ChildItem Cert:\\CurrentUser\\My\\{cert.Thumbprint}) -FilePath ArcDeck.cer");
            Console.WriteLine("    Import-Certificate -FilePath ArcDeck.cer -CertStoreLocation Cert:\\LocalMachine\\Root");
            Console.WriteLine();
            Console.WriteLine("    # 然后回来跑：");
            Console.WriteLine("    Add-AppxPackage -Path <msix>");
            Console.WriteLine("    Start-Process \"shell:appsFolder\\ProjectArcturius.ArcDeck_<hash>!App\"");
        }
    }

    static X509Certificate2 FindCert()
    {
        using var s = new X509Store(StoreName.My, StoreLocation.CurrentUser);
        s.Open(OpenFlags.ReadOnly);
        foreach (var c in s.Certificates)
        {
            if (c.Subject == Subject && c.NotAfter > DateTime.Now) return c;
        }
        return null;
    }

    static X509Certificate2 CreateCert()
    {
        try
        {
            using var rsa = RSA.Create(2048);

            var req = new CertificateRequest(Subject, rsa, HashAlgorithmName.SHA256,
                RSASignaturePadding.Pkcs1);

            req.CertificateExtensions.Add(new X509BasicConstraintsExtension(false, false, 0, false));
            req.CertificateExtensions.Add(new X509KeyUsageExtension(
                X509KeyUsageFlags.DigitalSignature, false));
            req.CertificateExtensions.Add(new X509EnhancedKeyUsageExtension(
                new OidCollection { new Oid("1.3.6.1.5.5.7.3.3") }, false)); // Code Signing
            req.CertificateExtensions.Add(new X509SubjectKeyIdentifierExtension(req.PublicKey, false));

            var cert = req.CreateSelfSigned(
                DateTimeOffset.Now.AddDays(-1), DateTimeOffset.Now.AddYears(3));

            // 用 PFX 往返一次，得到带私钥、可导出的实例（否则签名时报无私钥）。
            var pfx = cert.Export(X509ContentType.Pfx, "arctmp");
            var withKey = new X509Certificate2(pfx, "arctmp",
                X509KeyStorageFlags.Exportable | X509KeyStorageFlags.PersistKeySet);

            using var store = new X509Store(StoreName.My, StoreLocation.CurrentUser);
            store.Open(OpenFlags.ReadWrite);
            store.Add(withKey);

            return withKey;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"  建证书异常: {ex.Message}");
            return null;
        }
    }

    static bool Exists(StoreName name, string thumbprint)
    {
        try
        {
            using var s = new X509Store(name, StoreLocation.CurrentUser);
            s.Open(OpenFlags.ReadOnly);
            foreach (var c in s.Certificates)
            {
                if (string.Equals(c.Thumbprint, thumbprint, StringComparison.OrdinalIgnoreCase))
                    return true;
            }
        }
        catch { }
        return false;
    }

    static bool AddTo(StoreName name, X509Certificate2 cert)
    {
        try
        {
            using var s = new X509Store(name, StoreLocation.CurrentUser);
            s.Open(OpenFlags.ReadWrite);
            s.Add(new X509Certificate2(cert.Export(X509ContentType.Cert)));
            return true;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"    {ex.Message}");
            return false;
        }
    }

    static bool Sign(string msix, X509Certificate2 cert)
    {
        // signtool 是唯一现实可行的签名实现（MSIX 签名块格式不公开到可自写的程度）。
        var signtool = FindSigntool();
        if (signtool == null)
        {
            Console.WriteLine("  找不到 signtool.exe（在 Windows Kits\\10\\bin 下）");
            return false;
        }

        var psi = new System.Diagnostics.ProcessStartInfo(signtool)
        {
            Arguments = $"sign /fd SHA256 /sha1 {cert.Thumbprint} /a \"{msix}\"",
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };

        using var p = System.Diagnostics.Process.Start(psi);
        var o = p.StandardOutput.ReadToEnd();
        var e = p.StandardError.ReadToEnd();
        p.WaitForExit();

        if (!string.IsNullOrWhiteSpace(o)) Console.WriteLine("  " + o.Trim());
        if (p.ExitCode != 0 && !string.IsNullOrWhiteSpace(e)) Console.WriteLine("  " + e.Trim());
        return p.ExitCode == 0;
    }

    static string FindSigntool()
    {
        var kits = @"C:\Program Files (x86)\Windows Kits\10\bin";
        if (!Directory.Exists(kits)) return null;
        foreach (var d in Directory.GetDirectories(kits))
        {
            var p = Path.Combine(d, "x64", "signtool.exe");
            if (File.Exists(p)) return p;
        }
        return null;
    }

    static bool HasSignatureBlock(string msix)
    {
        try
        {
            using var zip = System.IO.Compression.ZipFile.OpenRead(msix);
            foreach (var e in zip.Entries)
            {
                if (e.FullName.Equals("AppxSignature.p7x", StringComparison.OrdinalIgnoreCase))
                    return true;
            }
        }
        catch { }
        return false;
    }

    static string FindMsix()
    {
        var dir = Directory.GetCurrentDirectory();
        for (var i = 0; i < 6 && dir != null; i++)
        {
            if (Directory.Exists(dir))
            {
                var hits = Directory.GetFiles(dir, "*.msix", SearchOption.AllDirectories);

                // 取**最新构建的那个**，不能取第一个。
                //
                // 这里出过一个真实的静默失败：AppPackages 下会同时留着多个版本
                // 的包（提版本号后旧目录不会自动清理），而 GetFiles 的返回顺序
                // 不保证是新的在前。结果是签名签在旧包上，新构建的包没签名，
                // 安装时报 0x80073CF0 / 0x800B0100（"必须经过数字签名"）——
                // 看起来像签名环节坏了，实际是签错了文件。
                if (hits.Length > 0)
                {
                    var newest = hits[0];
                    var newestTime = File.GetLastWriteTimeUtc(newest);
                    foreach (var h in hits)
                    {
                        var t = File.GetLastWriteTimeUtc(h);
                        if (t > newestTime)
                        {
                            newest = h;
                            newestTime = t;
                        }
                    }

                    if (hits.Length > 1)
                    {
                        Console.WriteLine($"  （目录下有 {hits.Length} 个 .msix，取最新的一个）");
                    }

                    return newest;
                }
            }
            dir = Path.GetDirectoryName(dir);
        }
        return null;
    }
}
